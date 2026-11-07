//! # sys_init — el arranque del espacio de usuario
//!
//! En la fase 3 este crate será el **PID 1** de KopitOS. Ya hoy contiene su
//! motor, que es lo difícil: un **grafo de servicios** que se resuelve en
//! *oleadas* — todos los servicios de una oleada no dependen entre sí y se
//! lanzan **en paralelo** (uno por núcleo libre), que es de donde sale un
//! arranque rápido.
//!
//! * Cada servicio declara qué necesita (`requires`) y en qué clases de
//!   dispositivo tiene sentido (`device_classes`): el compositor no se
//!   arranca en un servidor y el servicio de sensores sólo existe en IoT.
//! * El plan se valida entero **antes** de arrancar nada: una dependencia que
//!   falta o un ciclo se detectan al construir la distribución, no en el
//!   arranque de un dispositivo en el campo.
//! * `sandbox` declara el aislamiento que el núcleo aplicará al proceso.
//!
//! El núcleo usa hoy [`plan`] para mostrar y validar el plan de arranque del
//! perfil activo.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sandbox {
    /// Sin restricciones (sólo el propio init).
    None,
    /// Sin acceso a hardware; sólo los canales IPC declarados.
    Strict,
    /// Controlador: capacidades de MMIO/IRQ/DMA concedidas una a una.
    Driver,
}

#[derive(Clone, Copy, Debug)]
pub struct Service {
    pub name: &'static str,
    pub description: &'static str,
    pub requires: &'static [&'static str],
    /// Vacío = todas las clases de dispositivo.
    pub device_classes: &'static [&'static str],
    /// Si falla, el sistema entra en modo de rescate.
    pub critical: bool,
    pub sandbox: Sandbox,
}

impl Service {
    pub fn applies_to(&self, class: &str) -> bool {
        self.device_classes.is_empty() || self.device_classes.contains(&class)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// `service` necesita `dependency`, que no existe para esta clase de dispositivo.
    Missing { service: &'static str, dependency: &'static str },
    /// Los servicios restantes forman un ciclo.
    Cycle(Vec<&'static str>),
}

/// Oleadas de arranque: `plan[0]` arranca primero (en paralelo), etc.
pub type Plan = Vec<Vec<&'static Service>>;

/// Resuelve el orden de arranque para una clase de dispositivo (Kahn por niveles).
pub fn plan(services: &'static [Service], device_class: &str) -> Result<Plan, PlanError> {
    let active: Vec<&'static Service> = services.iter().filter(|s| s.applies_to(device_class)).collect();
    let index_of = |name: &str| active.iter().position(|s| s.name == name);

    // Grados de entrada y aristas dependencia → dependiente.
    let mut indegree = alloc::vec![0usize; active.len()];
    let mut dependents: Vec<Vec<usize>> = alloc::vec![Vec::new(); active.len()];
    for (i, s) in active.iter().enumerate() {
        for &dep in s.requires {
            let Some(d) = index_of(dep) else {
                return Err(PlanError::Missing { service: s.name, dependency: dep });
            };
            indegree[i] += 1;
            dependents[d].push(i);
        }
    }

    let mut done = alloc::vec![false; active.len()];
    let mut waves = Plan::new();
    let mut remaining = active.len();
    while remaining > 0 {
        let wave: Vec<usize> = (0..active.len()).filter(|&i| !done[i] && indegree[i] == 0).collect();
        if wave.is_empty() {
            let cycle = (0..active.len()).filter(|&i| !done[i]).map(|i| active[i].name).collect();
            return Err(PlanError::Cycle(cycle));
        }
        for &i in &wave {
            done[i] = true;
            remaining -= 1;
            for &j in &dependents[i] {
                indegree[j] -= 1;
            }
        }
        waves.push(wave.into_iter().map(|i| active[i]).collect());
    }
    Ok(waves)
}

const PERSONAL: &[&str] = &["desktop", "laptop", "tablet", "phone"];

/// Servicios de referencia de KopitOS (las distribuciones derivadas añaden los suyos).
pub static DEFAULT_SERVICES: &[Service] = &[
    Service {
        name: "logd",
        description: "registro del sistema",
        requires: &[],
        device_classes: &[],
        critical: true,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "devmgr",
        description: "gestor de dispositivos y controladores",
        requires: &["logd"],
        device_classes: &[],
        critical: true,
        sandbox: Sandbox::Driver,
    },
    Service {
        name: "kopitfs",
        description: "KopitFS: copy-on-write, sumas y réplicas",
        requires: &["devmgr"],
        device_classes: &[],
        critical: true,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "powerd",
        description: "energía adaptativa",
        requires: &["devmgr"],
        device_classes: &["laptop", "tablet", "phone", "iot"],
        critical: false,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "netd",
        description: "pila de red",
        requires: &["devmgr"],
        device_classes: &[],
        critical: false,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "timed",
        description: "hora de red",
        requires: &["netd"],
        device_classes: &[],
        critical: false,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "audiod",
        description: "audio",
        requires: &["devmgr"],
        device_classes: PERSONAL,
        critical: false,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "compositor",
        description: "compositor gráfico",
        requires: &["devmgr", "kopitfs"],
        device_classes: PERSONAL,
        critical: false,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "session",
        description: "sesión de usuario",
        requires: &["compositor", "audiod"],
        device_classes: PERSONAL,
        critical: false,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "sshd",
        description: "acceso remoto seguro",
        requires: &["netd", "kopitfs"],
        device_classes: &["server"],
        critical: false,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "sensord",
        description: "sensores y actuadores",
        requires: &["devmgr", "netd"],
        device_classes: &["iot"],
        critical: false,
        sandbox: Sandbox::Driver,
    },
    Service {
        name: "updated",
        description: "actualizaciones A/B firmadas",
        requires: &["netd", "kopitfs"],
        device_classes: &[],
        critical: false,
        sandbox: Sandbox::Strict,
    },
    Service {
        name: "shell",
        description: "consola de órdenes",
        requires: &["kopitfs"],
        device_classes: &[],
        critical: false,
        sandbox: Sandbox::None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: &Plan) -> Vec<Vec<&'static str>> {
        p.iter().map(|w| w.iter().map(|s| s.name).collect()).collect()
    }

    #[test]
    fn plan_de_escritorio() {
        let p = plan(DEFAULT_SERVICES, "desktop").unwrap();
        let n = names(&p);
        assert_eq!(n[0], vec!["logd"]);
        assert_eq!(n[1], vec!["devmgr"]);
        assert!(n[2].contains(&"kopitfs") && n[2].contains(&"netd") && n[2].contains(&"audiod"));
        assert!(!n.concat().contains(&"sshd"));
        assert!(!n.concat().contains(&"powerd"));
        // La sesión siempre arranca después del compositor.
        let pos = |s| n.iter().position(|w| w.contains(&s)).unwrap();
        assert!(pos("session") > pos("compositor"));
    }

    #[test]
    fn cada_dispositivo_su_plan() {
        let server = names(&plan(DEFAULT_SERVICES, "server").unwrap()).concat();
        assert!(server.contains(&"sshd") && !server.contains(&"compositor"));
        let iot = names(&plan(DEFAULT_SERVICES, "iot").unwrap()).concat();
        assert!(iot.contains(&"sensord") && iot.contains(&"powerd") && !iot.contains(&"session"));
    }

    #[test]
    fn detecta_dependencias_y_ciclos() {
        static FALTA: &[Service] = &[Service {
            name: "a",
            description: "",
            requires: &["b"],
            device_classes: &[],
            critical: false,
            sandbox: Sandbox::None,
        }];
        assert_eq!(plan(FALTA, "desktop").unwrap_err(), PlanError::Missing { service: "a", dependency: "b" });
        static CICLO: &[Service] = &[
            Service {
                name: "a",
                description: "",
                requires: &["b"],
                device_classes: &[],
                critical: false,
                sandbox: Sandbox::None,
            },
            Service {
                name: "b",
                description: "",
                requires: &["a"],
                device_classes: &[],
                critical: false,
                sandbox: Sandbox::None,
            },
            Service {
                name: "c",
                description: "",
                requires: &[],
                device_classes: &[],
                critical: false,
                sandbox: Sandbox::None,
            },
        ];
        assert_eq!(plan(CICLO, "desktop").unwrap_err(), PlanError::Cycle(vec!["a", "b"]));
    }
}
