// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal strict RFC 8259 JSON parser for `install-policy.json` with zero external crates.

use super::Result;

pub struct InstallPolicy {
    pub id: String,
    pub icon: String,
    pub sources: Vec<String>,
    pub extensions: Vec<String>,
    pub localization: Vec<String>,
    pub payloads: Vec<String>,
}

struct JsonCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonCursor<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn expect_byte(&mut self, b: u8) -> Result {
        self.skip_ws();
        if self.bytes.get(self.pos) != Some(&b) {
            return Err(format!("Expected '{}' at offset {}", b as char, self.pos).into());
        }
        self.pos += 1;
        Ok(())
    }

    fn read_u16_hex(&mut self) -> Result<u32> {
        let end = self.pos.checked_add(4).ok_or("JSON \\u offset overflow")?;
        let hex = std::str::from_utf8(self.bytes.get(self.pos..end).ok_or("Truncated \\u escape")?)?;
        self.pos = end;
        u32::from_str_radix(hex, 16).map_err(|e| format!("Invalid \\u escape: {e}").into())
    }

    fn decode_unicode_escape(&mut self) -> Result<char> {
        let code = self.read_u16_hex()?;
        if (0xDC00..=0xDFFF).contains(&code) {
            return Err("Unpaired low surrogate in JSON string".into());
        }
        if (0xD800..=0xDBFF).contains(&code) {
            if self.bytes.get(self.pos..self.pos + 2) != Some(b"\\u") {
                return Err("Unpaired high surrogate in JSON string".into());
            }
            self.pos += 2;
            let low = self.read_u16_hex()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err("Invalid low surrogate in JSON string".into());
            }
            let combined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
            return char::from_u32(combined).ok_or_else(|| "Invalid surrogate scalar".into());
        }
        char::from_u32(code).ok_or_else(|| "Invalid JSON unicode scalar".into())
    }

    fn read_escape(&mut self, out: &mut Vec<u8>) -> Result {
        let esc = *self.bytes.get(self.pos).ok_or("Unterminated JSON escape")?;
        self.pos += 1;
        match esc {
            b'"' | b'\\' | b'/' => out.push(esc),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0C),
            b'u' => {
                let mut enc = [0u8; 4];
                out.extend_from_slice(self.decode_unicode_escape()?.encode_utf8(&mut enc).as_bytes());
            }
            other => return Err(format!("Unsupported JSON escape \\{}", other as char).into()),
        }
        Ok(())
    }

    fn read_string(&mut self) -> Result<String> {
        self.expect_byte(b'"')?;
        let mut out = Vec::new();
        while let Some(&b) = self.bytes.get(self.pos) {
            self.pos += 1;
            match b {
                b'"' => return String::from_utf8(out).map_err(|e| format!("Invalid UTF-8 in JSON: {e}").into()),
                b'\\' => self.read_escape(&mut out)?,
                0x00..=0x1F => return Err("Unescaped control character in JSON string".into()),
                _ => out.push(b),
            }
        }
        Err("Unterminated JSON string".into())
    }

    fn read_string_array(&mut self) -> Result<Vec<String>> {
        self.expect_byte(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Ok(items);
        }
        loop {
            items.push(self.read_string()?);
            self.skip_ws();
            match self.bytes.get(self.pos).copied() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(items);
                }
                _ => return Err(format!("Malformed string array at offset {}", self.pos).into()),
            }
        }
    }

    fn skip_container(&mut self, close: u8, keyed: bool) -> Result {
        self.pos += 1;
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&close) {
            self.pos += 1;
            return Ok(());
        }
        loop {
            if keyed {
                let _ = self.read_string()?;
                self.expect_byte(b':')?;
            }
            self.skip_value()?;
            self.skip_ws();
            match self.bytes.get(self.pos).copied() {
                Some(b',') => self.pos += 1,
                Some(b) if b == close => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => return Err("Malformed JSON container".into()),
            }
        }
    }

    fn skip_literal(&mut self, expected: &[u8]) -> Result {
        let end = self.pos.checked_add(expected.len()).ok_or("JSON literal overflow")?;
        if self.bytes.get(self.pos..end) != Some(expected) {
            return Err("Invalid JSON literal".into());
        }
        self.pos = end;
        Ok(())
    }

    fn eat_digits(&mut self) -> Result {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start {
            return Err("Expected digit in JSON number".into());
        }
        Ok(())
    }

    fn skip_number(&mut self) -> Result {
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        match self.bytes.get(self.pos).copied() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => self.eat_digits()?,
            _ => return Err("Malformed JSON number".into()),
        }
        if self.bytes.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            self.eat_digits()?;
        }
        if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.bytes.get(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            self.eat_digits()?;
        }
        Ok(())
    }

    fn skip_value(&mut self) -> Result {
        self.skip_ws();
        match self.bytes.get(self.pos).copied() {
            Some(b'"') => self.read_string().map(|_| ()),
            Some(b'[') => self.skip_container(b']', false),
            Some(b'{') => self.skip_container(b'}', true),
            Some(b't') => self.skip_literal(b"true"),
            Some(b'f') => self.skip_literal(b"false"),
            Some(b'n') => self.skip_literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.skip_number(),
            Some(b) => Err(format!("Unexpected byte '{}' in JSON value", b as char).into()),
            None => Err("Unexpected EOF in JSON value".into()),
        }
    }
}

#[derive(Default)]
struct PolicyBuilder {
    id: Option<String>,
    icon: Option<String>,
    sources: Option<Vec<String>>,
    extensions: Option<Vec<String>>,
    localization: Option<Vec<String>>,
    payloads: Option<Vec<String>>,
}

impl PolicyBuilder {
    fn read_key(&mut self, cur: &mut JsonCursor<'_>, key: &str) -> Result {
        match key {
            "id" => self.id = Some(cur.read_string()?),
            "icon" => self.icon = Some(cur.read_string()?),
            "sources" => self.sources = Some(cur.read_string_array()?),
            "extensions" => self.extensions = Some(cur.read_string_array()?),
            "localization" => self.localization = Some(cur.read_string_array()?),
            "payloads" => self.payloads = Some(cur.read_string_array()?),
            _ => cur.skip_value()?,
        }
        Ok(())
    }

    fn finish(self) -> Result<InstallPolicy> {
        Ok(InstallPolicy {
            id: self.id.ok_or("Missing `id` in install-policy.json")?,
            icon: self.icon.ok_or("Missing `icon` in install-policy.json")?,
            sources: self.sources.ok_or("Missing `sources` in install-policy.json")?,
            extensions: self.extensions.ok_or("Missing `extensions` in install-policy.json")?,
            localization: self.localization.ok_or("Missing `localization` in install-policy.json")?,
            payloads: self.payloads.ok_or("Missing `payloads` in install-policy.json")?,
        })
    }
}

pub fn parse_install_policy(raw: &str) -> Result<InstallPolicy> {
    let mut cur = JsonCursor { bytes: raw.as_bytes(), pos: 0 };
    cur.expect_byte(b'{')?;
    let mut b = PolicyBuilder::default();
    cur.skip_ws();
    if cur.bytes.get(cur.pos) == Some(&b'}') {
        cur.pos += 1;
    } else {
        loop {
            let key = cur.read_string()?;
            cur.expect_byte(b':')?;
            b.read_key(&mut cur, &key)?;
            cur.skip_ws();
            match cur.bytes.get(cur.pos).copied() {
                Some(b',') => cur.pos += 1,
                Some(b'}') => {
                    cur.pos += 1;
                    break;
                }
                _ => return Err("Malformed top-level object in install-policy.json".into()),
            }
        }
    }
    cur.skip_ws();
    if cur.pos != cur.bytes.len() {
        return Err("Unexpected trailing data after top-level JSON object".into());
    }
    b.finish()
}
