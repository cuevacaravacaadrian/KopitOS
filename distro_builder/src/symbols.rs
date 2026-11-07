//! `kopit addr2line`: traduce direcciones de una traza de pánico a funciones.

use kopit_elf::Elf64;

/// Desenreda nombres Rust "legacy" (`_ZN4kopit3foo17h0123456789abcdefE`).
pub fn demangle(name: &str) -> String {
    let Some(mut rest) = name.strip_prefix("_ZN") else { return name.to_string() };
    let mut parts = Vec::new();
    while !rest.starts_with('E') && !rest.is_empty() {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Ok(n) = digits.parse::<usize>() else { return name.to_string() };
        rest = &rest[digits.len()..];
        if rest.len() < n {
            return name.to_string();
        }
        parts.push(rest[..n].to_string());
        rest = &rest[n..];
    }
    if parts.last().is_some_and(|p| p.len() == 17 && p.starts_with('h')) {
        parts.pop();
    }
    parts
        .join("::")
        .replace("$LT$", "<")
        .replace("$GT$", ">")
        .replace("$u20$", " ")
        .replace("$u7b$", "{")
        .replace("$u7d$", "}")
        .replace("$RF$", "&")
        .replace("$C$", ",")
        .replace("..", "::")
}

pub fn resolve(elf_data: &[u8], addrs: &[u64]) -> Result<Vec<(u64, Option<(String, u64)>)>, String> {
    let elf = Elf64::parse(elf_data).map_err(|e| e.message().to_string())?;
    let mut syms: Vec<(u64, u64, String)> = Vec::new();
    elf.for_each_symbol(|s| {
        if s.kind == 2 && s.value != 0 {
            syms.push((s.value, s.size, s.name.to_string()));
        }
    })
    .map_err(|e| e.message().to_string())?;
    syms.sort_by_key(|s| s.0);
    Ok(addrs
        .iter()
        .map(|&a| {
            let i = syms.partition_point(|s| s.0 <= a);
            let found = i.checked_sub(1).map(|i| &syms[i]).filter(|s| s.1 == 0 || a < s.0 + s.1);
            (a, found.map(|s| (demangle(&s.2), a - s.0)))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desenreda() {
        assert_eq!(
            demangle("_ZN12kopit_kernel4init11kernel_main17h0123456789abcdefE"),
            "kopit_kernel::init::kernel_main"
        );
        assert_eq!(demangle("kopit_entry_kbp"), "kopit_entry_kbp");
    }
}
