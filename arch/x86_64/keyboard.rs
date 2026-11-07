//! Teclado PS/2 en la IRQ 1 (controlador i8042 de `drivers/input/ps2`).

use super::idt::TrapFrame;
use super::io::Ports;
use crate::input::{self, Input};
use crate::sync::IrqLock;
use kopit_drv_ps2::{Decoder, I8042, Key, Layout};

static DECODER: IrqLock<Decoder> = IrqLock::new(Decoder::new(Layout::Es));

pub fn init(layout: Layout) -> Result<(), kopit_drv_ps2::Ps2Error> {
    DECODER.lock().set_layout(layout);
    I8042::new(Ports).init()?;
    super::apic::route_isa_irq(1, on_irq);
    Ok(())
}

pub fn set_layout(l: Layout) {
    DECODER.lock().set_layout(l);
}

pub fn layout() -> Layout {
    DECODER.lock().layout()
}

fn translate(key: Key, ctrl: bool) -> Option<Input> {
    Some(match key {
        Key::Char(c) if ctrl => Input::Ctrl(c.to_ascii_lowercase()),
        Key::Char(c) => Input::Char(c),
        Key::Enter => Input::Enter,
        Key::Backspace => Input::Backspace,
        Key::Tab => Input::Tab,
        Key::Escape => Input::Escape,
        Key::Up => Input::Up,
        Key::Down => Input::Down,
        Key::Left => Input::Left,
        Key::Right => Input::Right,
        Key::Home => Input::Home,
        Key::End => Input::End,
        Key::PageUp => Input::PageUp,
        Key::PageDown => Input::PageDown,
        Key::Delete => Input::Delete,
        Key::F(n) => Input::F(n),
        Key::Insert => return None,
    })
}

fn on_irq(_: &mut TrapFrame) {
    let controller = I8042::new(Ports);
    while let Some(sc) = controller.read_scancode() {
        if let Some(ev) = DECODER.lock().feed(sc) {
            if let Some(i) = translate(ev.key, ev.mods.ctrl) {
                input::push(i);
            }
        }
    }
    super::apic::eoi();
}
