//! Copyright and licence strings from an OpenType font's name table.
//!
//! Only notice metadata is read, using the fixed layouts in the
//! [OpenType naming table](https://learn.microsoft.com/en-us/typography/opentype/spec/name).

use std::mem::size_of;

/// The sfnt directory header and table entries have these fixed byte widths.
const DIRECTORY_HEADER: usize = 12;
const TABLE_RECORD: usize = 16;
/// Name-table headers and records have the same widths in versions zero and one.
const NAME_HEADER: usize = 6;
const NAME_RECORD: usize = 12;
/// The two OpenType name identifiers used in release notices.
const COPYRIGHT: usize = 0;
const LICENCE: usize = 13;

// Mac Roman's non-ASCII bytes, in byte order, from the Unicode vendor mapping:
// https://www.unicode.org/Public/MAPPINGS/VENDORS/APPLE/ROMAN.TXT.
const MAC_ROMAN: [char; 128] = [
    '\u{c4}', '\u{c5}', '\u{c7}', '\u{c9}', '\u{d1}', '\u{d6}', '\u{dc}', '\u{e1}', '\u{e0}',
    '\u{e2}', '\u{e4}', '\u{e3}', '\u{e5}', '\u{e7}', '\u{e9}', '\u{e8}', '\u{ea}', '\u{eb}',
    '\u{ed}', '\u{ec}', '\u{ee}', '\u{ef}', '\u{f1}', '\u{f3}', '\u{f2}', '\u{f4}', '\u{f6}',
    '\u{f5}', '\u{fa}', '\u{f9}', '\u{fb}', '\u{fc}', '\u{2020}', '\u{b0}', '\u{a2}', '\u{a3}',
    '\u{a7}', '\u{2022}', '\u{b6}', '\u{df}', '\u{ae}', '\u{a9}', '\u{2122}', '\u{b4}', '\u{a8}',
    '\u{2260}', '\u{c6}', '\u{d8}', '\u{221e}', '\u{b1}', '\u{2264}', '\u{2265}', '\u{a5}',
    '\u{b5}', '\u{2202}', '\u{2211}', '\u{220f}', '\u{3c0}', '\u{222b}', '\u{aa}', '\u{ba}',
    '\u{3a9}', '\u{e6}', '\u{f8}', '\u{bf}', '\u{a1}', '\u{ac}', '\u{221a}', '\u{192}', '\u{2248}',
    '\u{2206}', '\u{ab}', '\u{bb}', '\u{2026}', '\u{a0}', '\u{c0}', '\u{c3}', '\u{d5}', '\u{152}',
    '\u{153}', '\u{2013}', '\u{2014}', '\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}', '\u{f7}',
    '\u{25ca}', '\u{ff}', '\u{178}', '\u{2044}', '\u{20ac}', '\u{2039}', '\u{203a}', '\u{fb01}',
    '\u{fb02}', '\u{2021}', '\u{b7}', '\u{201a}', '\u{201e}', '\u{2030}', '\u{c2}', '\u{ca}',
    '\u{c1}', '\u{cb}', '\u{c8}', '\u{cd}', '\u{ce}', '\u{cf}', '\u{cc}', '\u{d3}', '\u{d4}',
    '\u{f8ff}', '\u{d2}', '\u{da}', '\u{db}', '\u{d9}', '\u{131}', '\u{2c6}', '\u{2dc}', '\u{af}',
    '\u{2d8}', '\u{2d9}', '\u{2da}', '\u{b8}', '\u{2dd}', '\u{2db}', '\u{2c7}',
];

fn span(bytes: &[u8], offset: usize, length: usize) -> Option<&[u8]> {
    bytes.get(offset..offset.checked_add(length)?)
}

fn word(bytes: &[u8], offset: usize) -> Option<usize> {
    Some(u16::from_be_bytes(span(bytes, offset, size_of::<u16>())?.try_into().ok()?).into())
}

fn dword(bytes: &[u8], offset: usize) -> Option<usize> {
    usize::try_from(u32::from_be_bytes(
        span(bytes, offset, size_of::<u32>())?.try_into().ok()?,
    ))
    .ok()
}

fn decode(platform: usize, encoding: usize, bytes: &[u8]) -> Option<String> {
    match (platform, encoding) {
        // Unicode, and Windows Symbol/BMP/full-repertoire strings, use UTF-16BE.
        (0, _) | (3, 0 | 1 | 10) => {
            if !bytes.len().is_multiple_of(size_of::<u16>()) {
                return None;
            }
            let words = bytes
                .chunks_exact(size_of::<u16>())
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            String::from_utf16(&words).ok()
        }
        (1, 0) => Some(
            bytes
                .iter()
                .map(|&byte| {
                    if byte.is_ascii() {
                        char::from(byte)
                    } else {
                        // A high byte is 0x80..=0xff, exactly the mapping's 128 entries.
                        MAC_ROMAN[usize::from(byte - 0x80)]
                    }
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Reads copyright and licence notices without interpreting any glyph or shaping tables.
pub(super) fn read(bytes: &[u8]) -> Option<Vec<String>> {
    match bytes.get(..size_of::<u32>())? {
        b"\0\x01\0\0" | b"OTTO" => {}
        _ => return None,
    }
    let tables = span(
        bytes,
        DIRECTORY_HEADER,
        word(bytes, size_of::<u32>())?.checked_mul(TABLE_RECORD)?,
    )?;
    let record = tables
        .chunks_exact(TABLE_RECORD)
        .find(|record| record.starts_with(b"name"))?;
    // A table entry is four words: tag, checksum, absolute offset and byte length.
    let table = span(
        bytes,
        dword(record, 2 * size_of::<u32>())?,
        dword(record, 3 * size_of::<u32>())?,
    )?;
    let version = word(table, 0)?;
    if version > 1 {
        return None;
    }
    let records = span(
        table,
        NAME_HEADER,
        word(table, size_of::<u16>())?.checked_mul(NAME_RECORD)?,
    )?;
    let storage = word(table, 2 * size_of::<u16>())?;
    let mut minimum_storage = NAME_HEADER.checked_add(records.len())?;
    if version == 1 {
        // Version one inserts a count and two-word language-tag records before string storage.
        let tags = word(table, minimum_storage)?;
        minimum_storage = minimum_storage
            .checked_add(size_of::<u16>())?
            .checked_add(tags.checked_mul(2 * size_of::<u16>())?)?;
    }
    if storage < minimum_storage || storage > table.len() {
        return None;
    }
    let mut names = Vec::new();
    for record in records.chunks_exact(NAME_RECORD) {
        // Name records contain platform, encoding, language, name id, length and relative offset.
        let id = word(record, 3 * size_of::<u16>())?;
        if id != COPYRIGHT && id != LICENCE {
            continue;
        }
        let offset = storage.checked_add(word(record, 5 * size_of::<u16>())?)?;
        let data = span(table, offset, word(record, 4 * size_of::<u16>())?)?;
        if let Some(name) = decode(word(record, 0)?, word(record, size_of::<u16>())?, data) {
            names.push(name);
        }
    }
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_be_bytes).collect()
    }

    fn font(records: &[(u16, u16, u16, Vec<u8>)], version: u16) -> Vec<u8> {
        let extra = if version == 1 { size_of::<u16>() } else { 0 };
        let storage = NAME_HEADER + records.len() * NAME_RECORD + extra;
        let mut table = Vec::new();
        table.extend(version.to_be_bytes());
        table.extend(u16::try_from(records.len()).unwrap().to_be_bytes());
        table.extend(u16::try_from(storage).unwrap().to_be_bytes());
        let mut strings = Vec::<u8>::new();
        for (platform, encoding, id, bytes) in records {
            for word in [
                *platform,
                *encoding,
                0,
                *id,
                u16::try_from(bytes.len()).unwrap(),
                u16::try_from(strings.len()).unwrap(),
            ] {
                table.extend(word.to_be_bytes());
            }
            strings.extend(bytes);
        }
        if version == 1 {
            table.extend(0u16.to_be_bytes());
        }
        table.extend(strings);
        let mut bytes = Vec::new();
        bytes.extend([0, 1, 0, 0]);
        bytes.extend(1u16.to_be_bytes());
        bytes.extend([0; 6]);
        bytes.extend(b"name");
        bytes.extend([0; 4]);
        bytes.extend(
            u32::try_from(DIRECTORY_HEADER + TABLE_RECORD)
                .unwrap()
                .to_be_bytes(),
        );
        bytes.extend(u32::try_from(table.len()).unwrap().to_be_bytes());
        bytes.extend(table);
        bytes
    }

    #[test]
    fn only_copyright_and_licence_records_are_decoded_in_each_supported_encoding() {
        let mut bytes = font(
            &[
                (0, 4, 0, utf16("Copyright © Example 🦀")),
                (3, 1, 13, utf16("SIL Open Font License")),
                (3, 10, 0, utf16("Full repertoire 𐐷")),
                (1, 0, 13, b"Caf\x8e \xa9 \xdb".to_vec()),
                (3, 2, 13, b"an unsupported code page".to_vec()),
                (0, 4, 1, utf16("a font family, not a notice")),
            ],
            0,
        );
        let expected = [
            "Copyright © Example 🦀",
            "SIL Open Font License",
            "Full repertoire 𐐷",
            "Café © €",
        ];
        assert_eq!(read(&bytes).unwrap(), expected);
        bytes[..4].copy_from_slice(b"OTTO");
        assert_eq!(read(&bytes).unwrap(), expected);
    }

    #[test]
    fn version_one_language_tags_do_not_change_the_name_record_layout() {
        let bytes = font(&[(0, 4, 13, utf16("notice in a version-one table"))], 1);
        assert_eq!(read(&bytes).unwrap(), ["notice in a version-one table"]);
    }

    #[test]
    fn truncated_headers_and_records_and_out_of_bounds_offsets_are_refused() {
        let bytes = font(&[(3, 1, 0, utf16("Copyright Example"))], 0);
        // Every prefix is shorter than this table's declared length; this small fixture stays below 100 bytes.
        for end in 0..bytes.len() {
            assert_eq!(read(&bytes[..end]), None, "prefix {end}");
        }
        let mut bad = bytes.clone();
        bad[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(read(&bad), None);
        let mut bad = bytes.clone();
        bad[24..28].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(read(&bad), None);
        let mut bad = bytes.clone();
        bad[32..34].copy_from_slice(&0u16.to_be_bytes());
        assert_eq!(
            read(&bad),
            None,
            "string storage cannot overlap name records"
        );
        let mut bad = bytes.clone();
        bad[44..46].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(read(&bad), None);
        let mut bad = bytes;
        bad[28..30].copy_from_slice(&2u16.to_be_bytes());
        assert_eq!(read(&bad), None, "unknown name-table version");
    }

    #[test]
    fn malformed_utf16_is_never_replaced_with_an_invented_notice() {
        let bytes = font(&[(3, 1, 0, vec![0xd8, 0]), (0, 3, 13, vec![b'a'])], 0);
        assert_eq!(read(&bytes), Some(Vec::new()));
    }

    #[test]
    fn the_installed_release_fonts_keep_their_original_notice_strings() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let fonts = root.join("frontend/node_modules/@electron-fonts/noto-sans-sc/fonts");
        if !fonts.is_dir() {
            eprintln!(
                "not run: install the locked frontend packages to check the release's real fonts"
            );
            return;
        }
        let expected = [
            "(c) 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'.",
            "This Font Software is licensed under the SIL Open Font License, Version 1.1. This license is available with a FAQ at: https://scripts.sil.org/OFL",
        ];
        for weight in [
            "Thin",
            "ExtraLight",
            "Light",
            "Regular",
            "Medium",
            "SemiBold",
            "Bold",
            "ExtraBold",
            "Black",
        ] {
            let path = fonts.join(format!("NotoSansSC-{weight}.ttf"));
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(read(&bytes).unwrap(), expected, "{}", path.display());
        }
    }
}
