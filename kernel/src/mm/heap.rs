//! # Montículo del núcleo (`alloc`)
//!
//! Dos caminos según el tamaño, como `kmalloc` en Linux:
//!
//! * **≤ 2 KiB → slabs**: 8 clases de tamaño (16…2048 B, potencias de 2). Cada
//!   clase tiene una lista de objetos libres; cuando se vacía, se trocea un
//!   marco de 4 KiB nuevo. Asignar y liberar son O(1) y los objetos quedan
//!   alineados a su tamaño (así se cumple cualquier `Layout::align` ≤ tamaño).
//! * **> 2 KiB → bloques buddy** de `2^n` páginas directamente del asignador
//!   físico, accedidos por el mapa directo: contiguos y sin fragmentar el
//!   espacio virtual.
//!
//! `GlobalAlloc::dealloc` recibe el mismo `Layout` que `alloc`, así que la
//! clase se recalcula sin guardar cabeceras: cero bytes de sobrecoste por
//! objeto.

use super::{PAGE_SIZE, phys_to_virt, pmm, virt_to_phys};
use crate::sync::IrqLock;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};
use kopit_buddy::order_for_pages;

pub const CLASSES: [usize; 8] = [16, 32, 64, 128, 256, 512, 1024, 2048];

struct Slabs {
    free: [usize; 8],
    in_use: [u64; 8],
    pages: [u64; 8],
    big_allocs: u64,
    big_pages: u64,
}

static SLABS: IrqLock<Slabs> =
    IrqLock::new(Slabs { free: [0; 8], in_use: [0; 8], pages: [0; 8], big_allocs: 0, big_pages: 0 });
static READY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Default)]
pub struct HeapStats {
    pub objects: [u64; 8],
    pub slab_pages: [u64; 8],
    pub big_allocs: u64,
    pub big_pages: u64,
}

impl HeapStats {
    pub fn total_bytes(&self) -> u64 {
        (self.slab_pages.iter().sum::<u64>() + self.big_pages) * PAGE_SIZE
    }
}

pub fn init() {
    READY.store(true, Ordering::Release);
}

pub fn is_ready() -> bool {
    READY.load(Ordering::Acquire)
}

pub fn stats() -> HeapStats {
    let s = SLABS.lock();
    HeapStats { objects: s.in_use, slab_pages: s.pages, big_allocs: s.big_allocs, big_pages: s.big_pages }
}

fn class_of(layout: &Layout) -> Option<usize> {
    let size = layout.size().max(layout.align()).max(16);
    CLASSES.iter().position(|&c| c >= size)
}

fn big_order(layout: &Layout) -> usize {
    let pages = (layout.size() as u64).div_ceil(PAGE_SIZE);
    let mut order = order_for_pages(pages);
    // Los bloques buddy de orden n están alineados a 4 KiB << n.
    while (PAGE_SIZE << order) < layout.align() as u64 {
        order += 1;
    }
    order
}

pub struct KernelAllocator;

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !is_ready() {
            return null_mut();
        }
        let Some(class) = class_of(&layout) else {
            let order = big_order(&layout);
            return match pmm::alloc_frames(order) {
                Some(f) => {
                    let mut s = SLABS.lock();
                    s.big_allocs += 1;
                    s.big_pages += 1 << order;
                    phys_to_virt(f) as *mut u8
                }
                None => null_mut(),
            };
        };
        let mut s = SLABS.lock();
        if s.free[class] == 0 {
            // Trocear un marco nuevo en objetos de esta clase.
            let Some(frame) = pmm::alloc_frame() else { return null_mut() };
            let base = phys_to_virt(frame) as usize;
            let size = CLASSES[class];
            let mut head = 0usize;
            for i in (0..PAGE_SIZE as usize / size).rev() {
                let obj = base + i * size;
                // SAFETY: objeto dentro del marco recién asignado.
                unsafe { (obj as *mut usize).write(head) };
                head = obj;
            }
            s.free[class] = head;
            s.pages[class] += 1;
        }
        let obj = s.free[class];
        // SAFETY: `obj` es un objeto libre de la lista; su primera palabra es el enlace.
        s.free[class] = unsafe { (obj as *const usize).read() };
        s.in_use[class] += 1;
        obj as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match class_of(&layout) {
            Some(class) => {
                let mut s = SLABS.lock();
                // SAFETY: el objeto vuelve a la lista; su primera palabra es el enlace.
                unsafe { (ptr as *mut usize).write(s.free[class]) };
                s.free[class] = ptr as usize;
                s.in_use[class] -= 1;
            }
            None => {
                let order = big_order(&layout);
                pmm::free_frames(virt_to_phys(ptr as u64), order);
                let mut s = SLABS.lock();
                s.big_allocs -= 1;
                s.big_pages -= 1 << order;
            }
        }
    }
}

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator;
