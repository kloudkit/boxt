use std::cmp::Ordering;
use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const BYTES_KEY: &str = "bytes_base64";

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Text(Repr);

#[derive(Clone, PartialEq, Eq, Hash)]
enum Repr {
    Utf8(String),
    Bytes(Vec<u8>),
}

impl Text {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(s) => Self(Repr::Utf8(s)),
            Err(e) => Self(Repr::Bytes(e.into_bytes())),
        }
    }

    pub fn from_wide(units: &[u16]) -> Self {
        let mut out = Vec::with_capacity(units.len());
        let mut i = 0;
        while i < units.len() {
            let u = u32::from(units[i]);
            let cp = if (0xD800..0xDC00).contains(&u)
                && i + 1 < units.len()
                && (0xDC00..0xE000).contains(&u32::from(units[i + 1]))
            {
                let low = u32::from(units[i + 1]);
                i += 1;
                0x10000 + ((u - 0xD800) << 10) + (low - 0xDC00)
            } else {
                u
            };
            i += 1;
            push_generalized_utf8(&mut out, cp);
        }
        Self::from_bytes(out)
    }

    pub fn as_bytes(&self) -> &[u8] {
        match &self.0 {
            Repr::Utf8(s) => s.as_bytes(),
            Repr::Bytes(b) => b,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.0 {
            Repr::Utf8(s) => Some(s),
            Repr::Bytes(_) => None,
        }
    }

    pub fn is_utf8(&self) -> bool {
        matches!(self.0, Repr::Utf8(_))
    }

    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        match self.0 {
            Repr::Utf8(s) => s.into_bytes(),
            Repr::Bytes(b) => b,
        }
    }
}

fn push_generalized_utf8(out: &mut Vec<u8>, cp: u32) {
    match cp {
        0..0x80 => out.push(cp as u8),
        0x80..0x800 => {
            out.push(0xC0 | (cp >> 6) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        }
        0x800..0x10000 => {
            out.push(0xE0 | (cp >> 12) as u8);
            out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        }
        _ => {
            out.push(0xF0 | (cp >> 18) as u8);
            out.push(0x80 | ((cp >> 12) & 0x3F) as u8);
            out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        }
    }
}

impl Default for Text {
    fn default() -> Self {
        Self(Repr::Utf8(String::new()))
    }
}

impl From<String> for Text {
    fn from(s: String) -> Self {
        Self(Repr::Utf8(s))
    }
}

impl From<&str> for Text {
    fn from(s: &str) -> Self {
        Self(Repr::Utf8(s.to_owned()))
    }
}

impl From<Vec<u8>> for Text {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_bytes(bytes)
    }
}

impl From<&[u8]> for Text {
    fn from(bytes: &[u8]) -> Self {
        Self::from_bytes(bytes.to_vec())
    }
}

impl PartialOrd for Text {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Text {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Utf8(s) => fmt::Debug::fmt(s, f),
            Repr::Bytes(b) => write!(f, "Text::bytes({b:02x?})"),
        }
    }
}

impl Serialize for Text {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Repr::Utf8(s) => serializer.serialize_str(s),
            Repr::Bytes(b) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(BYTES_KEY, &base64_encode(b))?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(TextVisitor)
    }
}

struct TextVisitor;

impl<'de> Visitor<'de> for TextVisitor {
    type Value = Text;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a string or {{\"{BYTES_KEY}\": …}}")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Text, E> {
        Ok(Text::from(v))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Text, E> {
        Ok(Text::from(v))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Text, A::Error> {
        let mut bytes = None;
        while let Some(key) = map.next_key::<String>()? {
            if key != BYTES_KEY {
                return Err(de::Error::unknown_field(&key, &[BYTES_KEY]));
            }
            if bytes.is_some() {
                return Err(de::Error::duplicate_field(BYTES_KEY));
            }
            let encoded: String = map.next_value()?;
            let decoded = base64_decode(&encoded)
                .ok_or_else(|| de::Error::custom("invalid base64 in Text"))?;
            bytes = Some(decoded);
        }
        bytes
            .map(Text::from_bytes)
            .ok_or_else(|| de::Error::missing_field(BYTES_KEY))
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(ALPHABET[(n >> 18) as usize & 0x3F]));
        out.push(char::from(ALPHABET[(n >> 12) as usize & 0x3F]));
        out.push(if chunk.len() > 1 {
            char::from(ALPHABET[(n >> 6) as usize & 0x3F])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(ALPHABET[n as usize & 0x3F])
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(encoded: &str) -> Option<Vec<u8>> {
    let input = encoded.as_bytes();
    if !input.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let quads = input.len() / 4;
    for (q, quad) in input.chunks(4).enumerate() {
        let last = q + 1 == quads;
        let pad = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &quad[..4 - pad] {
            n = (n << 6) | u32::from(sextet(c)?);
        }
        n <<= 6 * pad as u32;
        let produced = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        let keep = 3 - pad;
        if produced[keep..].iter().any(|&b| b != 0) {
            return None;
        }
        out.extend_from_slice(&produced[..keep]);
    }
    Some(out)
}

fn sextet(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_text_serializes_as_base64_object() {
        let t = Text::from_bytes(vec![0xff, 0x00]);
        assert!(!t.is_utf8());
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"bytes_base64":"/wA="}"#);
        assert_eq!(serde_json::from_str::<Text>(&json).unwrap(), t);
    }

    #[test]
    fn utf8_text_serializes_as_string() {
        let t = Text::from("héllo");
        assert_eq!(serde_json::to_string(&t).unwrap(), r#""héllo""#);
        let back: Text = serde_json::from_str(r#""héllo""#).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn valid_utf8_bytes_become_utf8() {
        let t = Text::from_bytes(b"abc".to_vec());
        assert!(t.is_utf8());
        let via_object: Text = serde_json::from_str(r#"{"bytes_base64":"YWJj"}"#).unwrap();
        assert_eq!(via_object, t);
        assert!(via_object.is_utf8());
    }

    #[test]
    fn raw_text_sorts_by_raw_bytes() {
        let mut v = [
            Text::from_bytes(vec![0xff, 0x00]),
            Text::from("z"),
            Text::from_bytes(vec![0x80]),
            Text::from("a"),
            Text::from("é"),
        ];
        v.sort();
        let bytes: Vec<&[u8]> = v.iter().map(Text::as_bytes).collect();
        assert_eq!(
            bytes,
            vec![
                b"a".as_slice(),
                b"z".as_slice(),
                &[0x80][..],
                "é".as_bytes(),
                &[0xff, 0x00][..],
            ]
        );
    }

    #[test]
    fn from_wide_gives_wtf8() {
        let ok: Vec<u16> = "a€😀".encode_utf16().collect();
        let t = Text::from_wide(&ok);
        assert_eq!(t.as_str(), Some("a€😀"));

        let lone = Text::from_wide(&[0x61, 0xD800, 0x62]);
        assert!(!lone.is_utf8());
        assert_eq!(lone.as_bytes(), &[0x61, 0xED, 0xA0, 0x80, 0x62]);

        let lone_low = Text::from_wide(&[0xDC00]);
        assert_eq!(lone_low.as_bytes(), &[0xED, 0xB0, 0x80]);
    }

    #[test]
    fn base64_round_trips_and_rejects_bad_input() {
        for len in 0..10u8 {
            let data: Vec<u8> = (0..len)
                .map(|i| i.wrapping_mul(37).wrapping_add(200))
                .collect();
            assert_eq!(base64_decode(&base64_encode(&data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert!(base64_decode("Zm8").is_none());
        assert!(base64_decode("Zm9=").is_none());
        assert!(base64_decode("Z===").is_none());
        assert!(base64_decode("Zm==Zm8=").is_none());
        assert!(base64_decode("Zm8*").is_none());
    }

    #[test]
    fn deserialize_rejects_unknown_keys() {
        assert!(serde_json::from_str::<Text>(r#"{"bytes":"AA=="}"#).is_err());
        assert!(serde_json::from_str::<Text>(r#"{}"#).is_err());
        assert!(serde_json::from_str::<Text>("12").is_err());
    }
}
