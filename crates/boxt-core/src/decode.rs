use crate::outcome::NotCheckedReason;
use crate::settings::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    pub encoding: Encoding,
}

const BOM_LE: [u8; 2] = [0xFF, 0xFE];
const BOM_BE: [u8; 2] = [0xFE, 0xFF];

pub fn decode(bytes: &[u8], declared: Option<Encoding>) -> Result<Decoded, NotCheckedReason> {
    let Some(encoding) = declared else {
        return undeclared(bytes);
    };
    decode_as(bytes, encoding)
        .map(|text| Decoded { text, encoding })
        .ok_or(NotCheckedReason::InvalidForDeclaredEncoding)
}

fn undeclared(bytes: &[u8]) -> Result<Decoded, NotCheckedReason> {
    if bytes.starts_with(&[0xFF, 0xFE, 0, 0]) || bytes.starts_with(&[0, 0, 0xFE, 0xFF]) {
        return Err(NotCheckedReason::EncodingUndeclared);
    }
    let found = if bytes.starts_with(&BOM_LE) {
        Some(Encoding::Utf16Le)
    } else if bytes.starts_with(&BOM_BE) {
        Some(Encoding::Utf16Be)
    } else {
        None
    };
    let encoding = found.unwrap_or(Encoding::Utf8);
    decode_as(bytes, encoding)
        .map(|text| Decoded { text, encoding })
        .ok_or(NotCheckedReason::EncodingUndeclared)
}

fn decode_as(bytes: &[u8], encoding: Encoding) -> Option<String> {
    match encoding {
        Encoding::Utf8 => utf8(bytes),
        Encoding::Utf16Le => utf16(bytes, encoding_rs::UTF_16LE, BOM_LE),
        Encoding::Utf16Be => utf16(bytes, encoding_rs::UTF_16BE, BOM_BE),
        Encoding::Latin1 => Some(encoding_rs::mem::decode_latin1(bytes).into_owned()),
        Encoding::WindowsCodePage(page) => code_page(bytes, page),
    }
}

fn utf8(bytes: &[u8]) -> Option<String> {
    String::from_utf8(bytes.to_vec()).ok()
}

fn utf16(bytes: &[u8], encoding: &'static encoding_rs::Encoding, bom: [u8; 2]) -> Option<String> {
    let body = bytes.strip_prefix(&bom).unwrap_or(bytes);
    whatwg(body, encoding)
}

fn whatwg(bytes: &[u8], encoding: &'static encoding_rs::Encoding) -> Option<String> {
    encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|text| text.into_owned())
}

fn code_page(bytes: &[u8], page: u16) -> Option<String> {
    match page {
        65001 => utf8(bytes),
        1200 => utf16(bytes, encoding_rs::UTF_16LE, BOM_LE),
        1201 => utf16(bytes, encoding_rs::UTF_16BE, BOM_BE),
        28591 => Some(encoding_rs::mem::decode_latin1(bytes).into_owned()),
        20127 => bytes
            .is_ascii()
            .then(|| encoding_rs::mem::decode_latin1(bytes).into_owned()),
        _ => whatwg(bytes, code_page_encoding(page)?),
    }
}

fn code_page_encoding(page: u16) -> Option<&'static encoding_rs::Encoding> {
    use encoding_rs as e;
    Some(match page {
        866 => e::IBM866,
        874 => e::WINDOWS_874,
        932 => e::SHIFT_JIS,
        936 => e::GBK,
        949 => e::EUC_KR,
        950 => e::BIG5,
        1250 => e::WINDOWS_1250,
        1251 => e::WINDOWS_1251,
        1252 => e::WINDOWS_1252,
        1253 => e::WINDOWS_1253,
        1254 => e::WINDOWS_1254,
        1255 => e::WINDOWS_1255,
        1256 => e::WINDOWS_1256,
        1257 => e::WINDOWS_1257,
        1258 => e::WINDOWS_1258,
        10000 => e::MACINTOSH,
        10007 => e::X_MAC_CYRILLIC,
        20866 => e::KOI8_R,
        20932 | 51932 => e::EUC_JP,
        21866 => e::KOI8_U,
        28592 => e::ISO_8859_2,
        28593 => e::ISO_8859_3,
        28594 => e::ISO_8859_4,
        28595 => e::ISO_8859_5,
        28596 => e::ISO_8859_6,
        28597 => e::ISO_8859_7,
        28598 => e::ISO_8859_8,
        28603 => e::ISO_8859_13,
        28605 => e::ISO_8859_15,
        38598 => e::ISO_8859_8_I,
        50220 => e::ISO_2022_JP,
        54936 => e::GB18030,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::line_count;

    fn lines(text: &str) -> u64 {
        line_count(text.as_bytes(), None)
    }

    #[test]
    fn undeclared_is_utf8_or_utf16_with_a_mark() {
        let d = decode("int a;\n// é\n".as_bytes(), None).unwrap();
        assert_eq!(d.encoding, Encoding::Utf8);
        assert_eq!(d.text, "int a;\n// é\n");

        let le = [0xFF, 0xFE, b'a', 0, b'\n', 0, b'b', 0];
        let d = decode(&le, None).unwrap();
        assert_eq!((d.text.as_str(), d.encoding), ("a\nb", Encoding::Utf16Le));
        assert_eq!(lines(&d.text), line_count(&le, None));

        let be = [0xFE, 0xFF, 0, b'x', 0, b'\n'];
        let d = decode(&be, None).unwrap();
        assert_eq!((d.text.as_str(), d.encoding), ("x\n", Encoding::Utf16Be));
        assert_eq!(lines(&d.text), line_count(&be, None));
    }

    #[test]
    fn undeclared_latin1_is_refused() {
        assert_eq!(
            decode(b"char *s = \"caf\xe9\";\n", None),
            Err(NotCheckedReason::EncodingUndeclared)
        );
        assert_eq!(
            decode(&[0xFF, 0xFE, b'a'], None),
            Err(NotCheckedReason::EncodingUndeclared)
        );
    }

    #[test]
    fn a_utf32_mark_is_undeclared() {
        let le = [0xFF, 0xFE, 0, 0, b'a', 0, 0, 0];
        assert_eq!(decode(&le, None), Err(NotCheckedReason::EncodingUndeclared));
        let be = [0, 0, 0xFE, 0xFF, 0, 0, 0, b'a'];
        assert_eq!(decode(&be, None), Err(NotCheckedReason::EncodingUndeclared));
    }

    #[test]
    fn declared_encodings_decode_or_are_invalid() {
        let latin = b"caf\xe9\n";
        let d = decode(latin, Some(Encoding::Latin1)).unwrap();
        assert_eq!(d.text, "café\n");
        assert_eq!(lines(&d.text), line_count(latin, Some(Encoding::Latin1)));

        assert_eq!(
            decode(latin, Some(Encoding::Utf8)),
            Err(NotCheckedReason::InvalidForDeclaredEncoding)
        );
        let d = decode(b"\x80\n", Some(Encoding::WindowsCodePage(1252))).unwrap();
        assert_eq!(d.text, "€\n");
        let sjis = b"\x82\xa0\n";
        assert_eq!(
            decode(sjis, Some(Encoding::WindowsCodePage(932)))
                .unwrap()
                .text,
            "あ\n"
        );
        assert_eq!(
            decode(b"\x82\n", Some(Encoding::WindowsCodePage(932))),
            Err(NotCheckedReason::InvalidForDeclaredEncoding)
        );
        assert_eq!(
            decode(b"a", Some(Encoding::WindowsCodePage(1))),
            Err(NotCheckedReason::InvalidForDeclaredEncoding)
        );
        assert_eq!(
            decode(b"\xe9", Some(Encoding::WindowsCodePage(20127))),
            Err(NotCheckedReason::InvalidForDeclaredEncoding)
        );
    }

    #[test]
    fn declared_utf16_drops_only_its_own_mark() {
        let le = [0xFF, 0xFE, b'a', 0, b'\n', 0];
        let d = decode(&le, Some(Encoding::Utf16Le)).unwrap();
        assert_eq!(d.text, "a\n");
        assert_eq!(lines(&d.text), line_count(&le, Some(Encoding::Utf16Le)));
        let cp = decode(&le, Some(Encoding::WindowsCodePage(1200))).unwrap();
        assert_eq!(cp.text, "a\n");
        let odd = [b'a', 0, b'b'];
        assert_eq!(
            decode(&odd, Some(Encoding::Utf16Le)),
            Err(NotCheckedReason::InvalidForDeclaredEncoding)
        );
    }

    #[test]
    fn a_utf8_mark_is_kept_so_lines_match() {
        let bytes = b"\xEF\xBB\xBF";
        let d = decode(bytes, None).unwrap();
        assert_eq!(d.text, "\u{FEFF}");
        assert_eq!(lines(&d.text), line_count(bytes, None));
    }

    #[test]
    fn crlf_lines_match_the_byte_count() {
        let bytes = b"a\r\nb\r\n\r\nc";
        let d = decode(bytes, None).unwrap();
        assert_eq!(lines(&d.text), line_count(bytes, None));
    }
}
