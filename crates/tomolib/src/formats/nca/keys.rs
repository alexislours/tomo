use std::collections::HashMap;
use std::fmt::Write as _;

use crate::formats::nca::crypto::{Aes128Ecb, aes128_ecb_decrypt};
use crate::{Error, Result};

const KEAK_NAMES: [&str; 3] = [
    "key_area_key_application_",
    "key_area_key_ocean_",
    "key_area_key_system_",
];

#[derive(Debug, Clone)]
pub struct KeySet {
    header_data: Aes128Ecb,
    header_tweak: Aes128Ecb,
    key_area_keys: [HashMap<u8, [u8; 16]>; 3],
    titlekek: HashMap<u8, [u8; 16]>,
    title_keys: HashMap<[u8; 16], [u8; 16]>,
}

impl KeySet {
    pub fn parse(prod_keys: &str, title_keys: Option<&str>) -> Result<Self> {
        let map = parse_key_lines(prod_keys);

        let header_key = map
            .get("header_key")
            .ok_or_else(|| Error::unsupported("prod.keys is missing `header_key`"))?;
        let header_key = parse_hex_32(header_key, "header_key")?;
        let mut data = [0u8; 16];
        let mut tweak = [0u8; 16];
        data.copy_from_slice(&header_key[..16]);
        tweak.copy_from_slice(&header_key[16..]);

        let mut key_area_keys: [HashMap<u8, [u8; 16]>; 3] = Default::default();
        for (slot, prefix) in KEAK_NAMES.iter().enumerate() {
            for (name, value) in &map {
                if let Some(rev) = name.strip_prefix(prefix)
                    && let Ok(generation) = u8::from_str_radix(rev, 16)
                {
                    key_area_keys[slot].insert(generation, parse_hex_16(value, name)?);
                }
            }
        }

        let mut titlekek = HashMap::new();
        for (name, value) in &map {
            if let Some(rev) = name.strip_prefix("titlekek_")
                && let Ok(generation) = u8::from_str_radix(rev, 16)
            {
                titlekek.insert(generation, parse_hex_16(value, name)?);
            }
        }

        let title_keys = match title_keys {
            Some(text) => parse_title_keys(text)?,
            None => HashMap::new(),
        };

        Ok(Self {
            header_data: Aes128Ecb::new(&data),
            header_tweak: Aes128Ecb::new(&tweak),
            key_area_keys,
            titlekek,
            title_keys,
        })
    }

    #[must_use]
    pub(crate) fn header_ciphers(&self) -> (&Aes128Ecb, &Aes128Ecb) {
        (&self.header_data, &self.header_tweak)
    }

    pub(crate) fn decrypt_key_area_key(
        &self,
        kaek_index: usize,
        generation: u8,
        wrapped: &[u8; 16],
    ) -> Result<[u8; 16]> {
        let bank = self.key_area_keys.get(kaek_index).ok_or_else(|| {
            Error::unsupported(format!(
                "unknown key area encryption key index {kaek_index}"
            ))
        })?;
        let kek = bank.get(&generation).ok_or_else(|| {
            Error::unsupported(format!(
                "missing key_area_key (index {kaek_index}, generation {generation:#04x})"
            ))
        })?;
        Ok(aes128_ecb_decrypt(kek, wrapped))
    }

    pub(crate) fn decrypt_title_key(
        &self,
        rights_id: &[u8; 16],
        generation: u8,
    ) -> Result<[u8; 16]> {
        let wrapped = self.title_keys.get(rights_id).ok_or_else(|| {
            Error::unsupported(format!(
                "title.keys has no entry for rights id {}",
                hex_lower(rights_id)
            ))
        })?;
        let kek = self.titlekek.get(&generation).ok_or_else(|| {
            Error::unsupported(format!("missing titlekek for generation {generation:#04x}"))
        })?;
        Ok(aes128_ecb_decrypt(kek, wrapped))
    }
}

fn parse_key_lines(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            map.insert(
                name.trim().to_ascii_lowercase(),
                value.trim().to_ascii_lowercase(),
            );
        }
    }
    map
}

fn parse_title_keys(text: &str) -> Result<HashMap<[u8; 16], [u8; 16]>> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some((rights_id, key)) = line.split_once('=') {
            let rights_id = rights_id.trim();
            let key = key.trim();
            if rights_id.len() != 32 || key.len() != 32 {
                continue;
            }
            map.insert(
                parse_hex_16(rights_id, "rights id")?,
                parse_hex_16(key, "title key")?,
            );
        }
    }
    Ok(map)
}

fn parse_hex_16(s: &str, ctx: &str) -> Result<[u8; 16]> {
    let bytes = decode_hex(s, ctx)?;
    bytes
        .try_into()
        .map_err(|_| Error::decode(format!("{ctx} must be 16 bytes")))
}

fn parse_hex_32(s: &str, ctx: &str) -> Result<[u8; 32]> {
    let bytes = decode_hex(s, ctx)?;
    bytes
        .try_into()
        .map_err(|_| Error::decode(format!("{ctx} must be 32 bytes")))
}

fn decode_hex(s: &str, ctx: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err(Error::decode(format!("{ctx} has odd hex length")));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| Error::decode(format!("{ctx} is not valid hex")))
        })
        .collect()
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
        header_key = 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\n\
        key_area_key_application_12 = 202122232425262728292a2b2c2d2e2f # comment\n\
        titlekek_12 = 303132333435363738393a3b3c3d3e3f\n";

    #[test]
    fn parses_and_unwraps_keys() {
        let keys = KeySet::parse(
            SAMPLE,
            Some("01006a800016e8000000000000000007 = 404142434445464748494a4b4c4d4e4f\n"),
        )
        .unwrap();
        let wrapped = [0u8; 16];
        keys.decrypt_key_area_key(0, 0x12, &wrapped).unwrap();
        assert!(keys.decrypt_key_area_key(1, 0x12, &wrapped).is_err());
        assert!(keys.decrypt_key_area_key(0, 0x05, &wrapped).is_err());
    }

    #[test]
    fn unwraps_title_key() {
        let keys = KeySet::parse(
            SAMPLE,
            Some("01006a800016e8000000000000000007 = 404142434445464748494a4b4c4d4e4f\n"),
        )
        .unwrap();

        let rights_id = parse_hex_16("01006a800016e8000000000000000007", "rights id").unwrap();
        let titlekek = parse_hex_16("303132333435363738393a3b3c3d3e3f", "titlekek").unwrap();
        let wrapped = parse_hex_16("404142434445464748494a4b4c4d4e4f", "title key").unwrap();
        let expected = aes128_ecb_decrypt(&titlekek, &wrapped);

        assert_eq!(keys.decrypt_title_key(&rights_id, 0x12).unwrap(), expected);
        assert!(keys.decrypt_title_key(&rights_id, 0x05).is_err());
        assert!(keys.decrypt_title_key(&[0u8; 16], 0x12).is_err());
    }

    #[test]
    fn rejects_missing_header_key() {
        assert!(KeySet::parse("key_area_key_application_00 = 00\n", None).is_err());
    }
}
