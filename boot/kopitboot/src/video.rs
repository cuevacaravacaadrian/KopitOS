//! Modo de vídeo con GOP (Graphics Output Protocol).
//!
//! El núcleo no cambia de modo (eso exige un controlador de GPU): hereda el
//! framebuffer que deja el cargador. Por eso aquí se elige con cuidado.

use crate::config::Resolution;
use crate::firmware::locate_protocol;
use crate::uefi::*;
use kopit_boot_proto::{Framebuffer, FramebufferKind};

pub fn setup(res: Resolution) -> Option<Framebuffer> {
    let gop: *mut GraphicsOutput = locate_protocol(&GRAPHICS_OUTPUT_GUID).ok()?;
    // SAFETY: protocolo GOP válido; `mode` e `info` los mantiene el firmware.
    unsafe {
        let mode = &*(*gop).mode;
        let current = mode.mode;
        if res != Resolution::Keep {
            let mut best: Option<(u32, u32, u32)> = None; // (modo, ancho, alto)
            let mut exact = None;
            for i in 0..mode.max_mode {
                let mut size = 0usize;
                let mut info: *const GraphicsModeInfo = core::ptr::null();
                if ((*gop).query_mode)(gop, i, &mut size, &mut info).is_error() || info.is_null() {
                    continue;
                }
                let inf = *info;
                if inf.pixel_format > PIXEL_BITMASK {
                    continue; // sólo-BLT: no hay framebuffer lineal
                }
                let (w, h) = (inf.horizontal_resolution, inf.vertical_resolution);
                if res == Resolution::Exact(w, h) {
                    exact = Some(i);
                    break;
                }
                let fits = res == Resolution::Max || (w <= 1920 && h <= 1080);
                let area = w as u64 * h as u64;
                if fits && best.is_none_or(|(_, bw, bh)| area > bw as u64 * bh as u64) {
                    best = Some((i, w, h));
                }
            }
            let target = exact.or(best.map(|b| b.0));
            if let Some(t) = target.filter(|&t| t != current) {
                let _ = ((*gop).set_mode)(gop, t);
            }
        }

        let mode = &*(*gop).mode;
        let info = &*mode.info;
        let (rs, rb, gs, gb, bs, bb) = match info.pixel_format {
            PIXEL_RGB => (0, 8, 8, 8, 16, 8),
            PIXEL_BGR => (16, 8, 8, 8, 0, 8),
            PIXEL_BITMASK => (
                info.red_mask.trailing_zeros() as u8,
                info.red_mask.count_ones() as u8,
                info.green_mask.trailing_zeros() as u8,
                info.green_mask.count_ones() as u8,
                info.blue_mask.trailing_zeros() as u8,
                info.blue_mask.count_ones() as u8,
            ),
            _ => return None,
        };
        Some(Framebuffer {
            base: mode.frame_buffer_base,
            size: mode.frame_buffer_size as u64,
            width: info.horizontal_resolution,
            height: info.vertical_resolution,
            stride: info.pixels_per_scan_line * 4,
            bpp: 32,
            kind: FramebufferKind::RGB,
            red_shift: rs,
            red_bits: rb,
            green_shift: gs,
            green_bits: gb,
            blue_shift: bs,
            blue_bits: bb,
            _pad: [0; 2],
        })
    }
}
