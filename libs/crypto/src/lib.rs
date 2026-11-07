//! # Primitivas criptográficas y de integridad de KopitOS
//!
//! * [`sha256`]: huella del núcleo. `distro_builder` la calcula al construir la
//!   imagen y la **integra dentro del cargador** (que es el binario que firma
//!   Secure Boot); el cargador la recalcula antes de saltar al núcleo. Así la
//!   cadena de confianza firmware → cargador → núcleo no depende de ningún
//!   archivo de configuración modificable.
//! * [`crc32`]: la suma de comprobación IEEE que exigen las tablas GPT.
//!
//! Todo es `no_std`, sin `unsafe` y sin dependencias, y se prueba en el
//! anfitrión contra vectores oficiales.
//!
//! Próximo paso (fase 2): firmas Ed25519 para módulos y actualizaciones.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    const H0: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

    /// SHA-256 incremental.
    #[derive(Clone)]
    pub struct Sha256 {
        state: [u32; 8],
        block: [u8; 64],
        used: usize,
        total: u64,
    }

    impl Default for Sha256 {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Sha256 {
        pub const fn new() -> Self {
            Self { state: H0, block: [0; 64], used: 0, total: 0 }
        }

        pub fn update(&mut self, mut data: &[u8]) {
            self.total = self.total.wrapping_add(data.len() as u64);
            if self.used > 0 {
                let take = (64 - self.used).min(data.len());
                self.block[self.used..self.used + take].copy_from_slice(&data[..take]);
                self.used += take;
                data = &data[take..];
                if self.used == 64 {
                    let block = self.block;
                    compress(&mut self.state, &block);
                    self.used = 0;
                }
            }
            while data.len() >= 64 {
                let (chunk, rest) = data.split_at(64);
                let mut block = [0u8; 64];
                block.copy_from_slice(chunk);
                compress(&mut self.state, &block);
                data = rest;
            }
            if !data.is_empty() {
                self.block[..data.len()].copy_from_slice(data);
                self.used = data.len();
            }
        }

        pub fn finalize(mut self) -> [u8; 32] {
            let bits = self.total.wrapping_mul(8);
            let mut pad = [0u8; 72];
            pad[0] = 0x80;
            // Relleno hasta 56 (mod 64) y luego la longitud en bits (big-endian).
            let pad_len = if self.used < 56 { 56 - self.used } else { 120 - self.used };
            pad[pad_len..pad_len + 8].copy_from_slice(&bits.to_be_bytes());
            let total = self.total;
            self.update(&pad[..pad_len + 8]);
            self.total = total;
            debug_assert_eq!(self.used, 0);
            let mut out = [0u8; 32];
            for (i, w) in self.state.iter().enumerate() {
                out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
            }
            out
        }
    }

    fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }

    /// Huella SHA-256 de un bloque de datos.
    pub fn digest(data: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(data);
        h.finalize()
    }

    /// Hexadecimal en minúsculas (64 caracteres ASCII).
    pub fn to_hex(hash: &[u8; 32]) -> [u8; 64] {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = [0u8; 64];
        for (i, b) in hash.iter().enumerate() {
            out[i * 2] = HEX[(b >> 4) as usize];
            out[i * 2 + 1] = HEX[(b & 0xF) as usize];
        }
        out
    }

    /// Lee una huella en hexadecimal (mayúsculas o minúsculas).
    pub fn from_hex(s: &str) -> Option<[u8; 32]> {
        let s = s.trim().as_bytes();
        if s.len() != 64 {
            return None;
        }
        let nibble = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        };
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = (nibble(s[i * 2])? << 4) | nibble(s[i * 2 + 1])?;
        }
        Some(out)
    }
}

pub mod crc32 {
    /// Tabla del polinomio IEEE 802.3 (reflejado: 0xEDB88320), calculada al compilar.
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };

    /// CRC32 continuable: `update(update(0, a), b) == crc32(a ++ b)`.
    pub fn update(crc: u32, data: &[u8]) -> u32 {
        let mut c = !crc;
        for &b in data {
            c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        !c
    }

    pub fn checksum(data: &[u8]) -> u32 {
        update(0, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(h: [u8; 32]) -> String {
        String::from_utf8(sha256::to_hex(&h).to_vec()).unwrap()
    }

    #[test]
    fn sha256_vectores_oficiales() {
        assert_eq!(hex(sha256::digest(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(sha256::digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            hex(sha256::digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_incremental_igual_que_de_golpe() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i * 31 % 251) as u8).collect();
        for split in [0usize, 1, 55, 56, 63, 64, 65, 4097] {
            let mut h = sha256::Sha256::new();
            h.update(&data[..split]);
            h.update(&data[split..]);
            assert_eq!(h.finalize(), sha256::digest(&data), "corte en {split}");
        }
        let million_a = vec![b'a'; 1_000_000];
        assert_eq!(hex(sha256::digest(&million_a)), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn hex_ida_y_vuelta() {
        let h = sha256::digest(b"KopitOS");
        let s = sha256::to_hex(&h);
        assert_eq!(sha256::from_hex(core::str::from_utf8(&s).unwrap()), Some(h));
        assert_eq!(sha256::from_hex("zz"), None);
    }

    #[test]
    fn crc32_vector_estandar() {
        assert_eq!(crc32::checksum(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32::update(crc32::update(0, b"1234"), b"56789"), 0xCBF4_3926);
    }
}
