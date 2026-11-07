//! # Compositor de KopitOS (núcleo de la escena)
//!
//! Es la parte **independiente de la GPU** del compositor: decide *qué* hay
//! que volver a pintar y *en qué orden*; el *cómo* lo pone un
//! [`RenderBackend`]. Fase 1 incluye el modelo y sus pruebas; el backend por
//! software se conecta en la fase 3 y los backends acelerados después.
//!
//! ## Ideas clave
//!
//! * **Daños** ([`Damage`]): sólo se recompone lo que cambió. Los rectángulos
//!   cercanos se fusionan y, si hay demasiados, se colapsan en su envolvente
//!   (dibujar un poco de más es más barato que muchas pasadas pequeñas).
//! * **Oclusión**: una capa opaca tapa a las de debajo; no se dibujan.
//! * **Backends intercambiables**: `Software` (siempre disponible, también en
//!   IoT sin GPU), `Vulkan` (Linux/Android-like y virtio-gpu Venus), `Metal`
//!   (hardware Apple) y `OpenGl` (compatibilidad). [`choose_backend`] elige
//!   según lo que haya y la clase de dispositivo (en un teléfono se prefiere
//!   ahorrar energía; en un escritorio, rendimiento).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }
    pub const fn right(&self) -> i32 {
        self.x + self.w as i32
    }
    pub const fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
    pub const fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }

    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    pub fn contains(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }
}

/// Acumulador de zonas dañadas.
pub struct Damage {
    rects: Vec<Rect>,
    max_rects: usize,
}

impl Damage {
    pub fn new(max_rects: usize) -> Self {
        Self { rects: Vec::new(), max_rects: max_rects.max(1) }
    }

    pub fn add(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        let mut r = r;
        // Fusionar con todo lo que se solape o cuya unión desperdicie poco.
        let mut i = 0;
        while i < self.rects.len() {
            let o = self.rects[i];
            let u = r.union(&o);
            if r.intersect(&o).is_some() || u.area() <= (r.area() + o.area()) * 5 / 4 {
                r = u;
                self.rects.swap_remove(i);
                i = 0;
            } else {
                i += 1;
            }
        }
        self.rects.push(r);
        if self.rects.len() > self.max_rects {
            let all = self.rects.iter().fold(Rect::default(), |a, b| a.union(b));
            self.rects.clear();
            self.rects.push(all);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }

    pub fn take(&mut self) -> Vec<Rect> {
        core::mem::take(&mut self.rects)
    }
}

pub type LayerId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layer {
    pub id: LayerId,
    pub rect: Rect,
    pub z: i32,
    pub opacity: u8,
    pub visible: bool,
}

impl Layer {
    pub fn opaque(&self) -> bool {
        self.visible && self.opacity == 255
    }
}

/// Lo que debe saber hacer cualquier backend gráfico.
pub trait RenderBackend {
    fn kind(&self) -> BackendKind;
    fn begin_frame(&mut self, damage: &[Rect]);
    /// Dibuja la parte `clip` de `layer`.
    fn draw_layer(&mut self, layer: &Layer, clip: Rect);
    fn end_frame(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Software,
    Vulkan,
    Metal,
    OpenGl,
}

impl BackendKind {
    pub const fn name(self) -> &'static str {
        match self {
            BackendKind::Software => "software",
            BackendKind::Vulkan => "Vulkan",
            BackendKind::Metal => "Metal",
            BackendKind::OpenGl => "OpenGL",
        }
    }
}

/// Elige backend según disponibilidad y clase de dispositivo.
pub fn choose_backend(available: &[BackendKind], device_class: &str) -> BackendKind {
    let preference: &[BackendKind] = match device_class {
        // En batería, Metal/Vulkan consumen menos que componer por CPU.
        "phone" | "tablet" | "laptop" => &[BackendKind::Metal, BackendKind::Vulkan, BackendKind::OpenGl],
        "desktop" => &[BackendKind::Vulkan, BackendKind::Metal, BackendKind::OpenGl],
        // Servidores e IoT: sin GPU o sin necesidad de ella.
        _ => &[],
    };
    preference.iter().copied().find(|p| available.contains(p)).unwrap_or(BackendKind::Software)
}

/// Escena: capas ordenadas por `z` y daño pendiente.
pub struct Scene {
    layers: Vec<Layer>,
    damage: Damage,
    next_id: LayerId,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    pub fn new() -> Self {
        Self { layers: Vec::new(), damage: Damage::new(16), next_id: 1 }
    }

    pub fn add_layer(&mut self, rect: Rect, z: i32, opacity: u8) -> LayerId {
        let id = self.next_id;
        self.next_id += 1;
        self.layers.push(Layer { id, rect, z, opacity, visible: true });
        self.layers.sort_by_key(|l| l.z);
        self.damage.add(rect);
        id
    }

    pub fn move_layer(&mut self, id: LayerId, rect: Rect) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.id == id) {
            self.damage.add(l.rect);
            l.rect = rect;
            self.damage.add(rect);
        }
    }

    pub fn set_visible(&mut self, id: LayerId, visible: bool) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.id == id) {
            l.visible = visible;
            self.damage.add(l.rect);
        }
    }

    pub fn invalidate(&mut self, r: Rect) {
        self.damage.add(r);
    }

    /// Recompone lo dañado. Devuelve cuántas capas se dibujaron.
    pub fn compose<B: RenderBackend>(&mut self, backend: &mut B) -> usize {
        if self.damage.is_empty() {
            return 0;
        }
        let damage = self.damage.take();
        backend.begin_frame(&damage);
        let mut drawn = 0;
        for d in &damage {
            // De arriba abajo: la primera capa opaca que cubre el daño tapa el resto.
            let mut first = 0;
            for (i, l) in self.layers.iter().enumerate().rev() {
                if l.opaque() && l.rect.contains(d) {
                    first = i;
                    break;
                }
            }
            for l in &self.layers[first..] {
                if !l.visible || l.opacity == 0 {
                    continue;
                }
                if let Some(clip) = l.rect.intersect(d) {
                    backend.draw_layer(l, clip);
                    drawn += 1;
                }
            }
        }
        backend.end_frame();
        drawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        drawn: Vec<(LayerId, Rect)>,
    }

    impl RenderBackend for Recorder {
        fn kind(&self) -> BackendKind {
            BackendKind::Software
        }
        fn begin_frame(&mut self, _: &[Rect]) {}
        fn draw_layer(&mut self, layer: &Layer, clip: Rect) {
            self.drawn.push((layer.id, clip));
        }
        fn end_frame(&mut self) {}
    }

    #[test]
    fn danos_se_fusionan() {
        let mut d = Damage::new(4);
        d.add(Rect::new(0, 0, 10, 10));
        d.add(Rect::new(5, 5, 10, 10));
        assert_eq!(d.rects(), &[Rect::new(0, 0, 15, 15)]);
        d.add(Rect::new(1000, 1000, 1, 1));
        assert_eq!(d.rects().len(), 2);
    }

    #[test]
    fn oclusion_por_capa_opaca() {
        let mut s = Scene::new();
        let fondo = s.add_layer(Rect::new(0, 0, 100, 100), 0, 255);
        let ventana = s.add_layer(Rect::new(10, 10, 50, 50), 1, 255);
        let mut r = Recorder::default();
        s.compose(&mut r);
        r.drawn.clear();
        // Sólo cambia algo DENTRO de la ventana opaca: el fondo no se repinta.
        s.invalidate(Rect::new(20, 20, 5, 5));
        s.compose(&mut r);
        assert_eq!(r.drawn, vec![(ventana, Rect::new(20, 20, 5, 5))]);
        // Al moverla, se repinta el fondo que descubrió.
        r.drawn.clear();
        s.move_layer(ventana, Rect::new(40, 40, 50, 50));
        s.compose(&mut r);
        assert!(r.drawn.iter().any(|&(id, _)| id == fondo));
    }

    #[test]
    fn eleccion_de_backend() {
        use BackendKind::*;
        assert_eq!(choose_backend(&[Vulkan, OpenGl], "desktop"), Vulkan);
        assert_eq!(choose_backend(&[Metal, Vulkan], "phone"), Metal);
        assert_eq!(choose_backend(&[Vulkan], "server"), Software);
        assert_eq!(choose_backend(&[], "desktop"), Software);
    }
}
