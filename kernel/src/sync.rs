//! # Primitivas de sincronización del núcleo
//!
//! * [`SpinLock`]: cerrojo de espera activa (sección crítica corta).
//! * [`IrqLock`]: además desactiva las interrupciones mientras está tomado.
//!   Es el que se usa para todo lo que también toca un manejador de
//!   interrupción (registro, consola, asignadores): si una interrupción
//!   intentara tomar un cerrojo que la CPU ya tiene, se bloquearía para siempre.
//! * [`Once`]: valor que se inicializa una vez y luego sólo se lee.
//! * [`Spsc`]: cola sin cerrojos de un productor (la IRQ) y un consumidor.
//! * [`RacyCell`]: datos globales que sólo toca el arranque, con un único hilo.
//!
//! Están pensadas para SMP desde el principio (atómicos con orden
//! Acquire/Release), aunque la fase 1 arranque un solo núcleo.

use crate::hal;
use core::cell::UnsafeCell;
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: el cerrojo serializa el acceso a `value`.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    pub fn lock(&self) -> SpinGuard<'_, T> {
        while self.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinGuard { lock: self }
    }

    pub fn try_lock(&self) -> Option<SpinGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinGuard { lock: self })
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    /// # Safety
    /// Sólo en un pánico: quien tenía el cerrojo no volverá a ejecutarse.
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: tenemos el cerrojo.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: tenemos el cerrojo.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

pub struct IrqLock<T> {
    inner: SpinLock<T>,
}

impl<T> IrqLock<T> {
    pub const fn new(value: T) -> Self {
        Self { inner: SpinLock::new(value) }
    }

    pub fn lock(&self) -> IrqGuard<'_, T> {
        let was_enabled = hal::disable_interrupts();
        IrqGuard { guard: ManuallyDrop::new(self.inner.lock()), was_enabled }
    }

    pub fn try_lock(&self) -> Option<IrqGuard<'_, T>> {
        let was_enabled = hal::disable_interrupts();
        match self.inner.try_lock() {
            Some(g) => Some(IrqGuard { guard: ManuallyDrop::new(g), was_enabled }),
            None => {
                hal::restore_interrupts(was_enabled);
                None
            }
        }
    }

    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }

    /// # Safety
    /// Ver [`SpinLock::force_unlock`].
    pub unsafe fn force_unlock(&self) {
        // SAFETY: lo garantiza quien llama.
        unsafe { self.inner.force_unlock() }
    }
}

pub struct IrqGuard<'a, T> {
    guard: ManuallyDrop<SpinGuard<'a, T>>,
    was_enabled: bool,
}

impl<T> Deref for IrqGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for IrqGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> Drop for IrqGuard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: se suelta una sola vez; primero el cerrojo, luego las interrupciones.
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        hal::restore_interrupts(self.was_enabled);
    }
}

const ONCE_EMPTY: u8 = 0;
const ONCE_BUSY: u8 = 1;
const ONCE_READY: u8 = 2;

pub struct Once<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: el valor sólo se escribe una vez, antes de publicarlo con Release.
unsafe impl<T: Send + Sync> Sync for Once<T> {}

impl<T> Once<T> {
    pub const fn new() -> Self {
        Self { state: AtomicU8::new(ONCE_EMPTY), value: UnsafeCell::new(MaybeUninit::uninit()) }
    }

    /// Inicializa (si nadie lo hizo) y devuelve el valor.
    pub fn call_once(&self, f: impl FnOnce() -> T) -> &T {
        if self.state.compare_exchange(ONCE_EMPTY, ONCE_BUSY, Ordering::Acquire, Ordering::Acquire).is_ok() {
            // SAFETY: somos los únicos en estado BUSY.
            unsafe { (*self.value.get()).write(f()) };
            self.state.store(ONCE_READY, Ordering::Release);
        }
        while self.state.load(Ordering::Acquire) != ONCE_READY {
            core::hint::spin_loop();
        }
        // SAFETY: estado READY ⇒ inicializado.
        unsafe { (*self.value.get()).assume_init_ref() }
    }

    pub fn get(&self) -> Option<&T> {
        // SAFETY: estado READY ⇒ inicializado.
        (self.state.load(Ordering::Acquire) == ONCE_READY).then(|| unsafe { (*self.value.get()).assume_init_ref() })
    }
}

/// Cola circular sin cerrojos: un productor (manejador de interrupción) y un
/// consumidor (el bucle principal). Si se llena, se descartan los nuevos.
pub struct Spsc<T: Copy, const N: usize> {
    buffer: UnsafeCell<[MaybeUninit<T>; N]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// SAFETY: productor y consumidor tocan posiciones distintas, publicadas con Release.
unsafe impl<T: Copy + Send, const N: usize> Sync for Spsc<T, N> {}

impl<T: Copy, const N: usize> Spsc<T, N> {
    pub const fn new() -> Self {
        Self {
            buffer: UnsafeCell::new([MaybeUninit::uninit(); N]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    pub fn push(&self, value: T) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let next = (tail + 1) % N;
        if next == self.head.load(Ordering::Acquire) {
            return false;
        }
        // SAFETY: sólo el productor escribe en `tail`.
        unsafe { (*self.buffer.get())[tail].write(value) };
        self.tail.store(next, Ordering::Release);
        true
    }

    pub fn pop(&self) -> Option<T> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: la posición `head` la publicó el productor.
        let v = unsafe { (*self.buffer.get())[head].assume_init() };
        self.head.store((head + 1) % N, Ordering::Release);
        Some(v)
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }
}

/// Dato global que sólo modifica el arranque (un hilo, sin interrupciones).
pub struct RacyCell<T>(UnsafeCell<T>);

// SAFETY: el uso está restringido por contrato a la fase de arranque.
unsafe impl<T> Sync for RacyCell<T> {}

impl<T> RacyCell<T> {
    pub const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    pub fn get(&self) -> *mut T {
        self.0.get()
    }
}

/// Marca de "una vez": devuelve `true` sólo la primera vez.
pub struct Flag(AtomicBool);

impl Flag {
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    pub fn set_first(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
