use std::io::Write;

use crate::Result;
use crate::formats::lms::Header;
use crate::formats::msbp::{self, Msbp};
use crate::formats::msbt::Msbt;

use super::registry::Registry;
use super::shared::{hex_encode, needs_escape, write_hex, write_quoted};

fn emit_header<W: Write>(w: &mut W, h: &Header) -> Result<()> {
    writeln!(w, "encoding: {}", encoding_name(h.encoding))?;
    writeln!(w, "version: {}", h.version)?;
    writeln!(w, "_meta:")?;
    writeln!(
        w,
        "  reserved_a: {}",
        yaml_quote(&hex_encode(&h.reserved_a))
    )?;
    writeln!(
        w,
        "  reserved_b: {}",
        yaml_quote(&hex_encode(&h.reserved_b))
    )?;
    writeln!(
        w,
        "  reserved_tail: {}",
        yaml_quote(&hex_encode(&h.reserved_tail))
    )?;
    Ok(())
}

fn emit_sections<W: Write>(w: &mut W, order: &[[u8; 4]]) -> Result<()> {
    let sections: Vec<String> = order
        .iter()
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    writeln!(w, "  sections: [{}]", sections.join(", "))?;
    Ok(())
}

fn encoding_name(e: u8) -> String {
    match e {
        0 => "utf8".into(),
        1 => "utf16le".into(),
        2 => "utf16be".into(),
        other => format!("raw{other}"),
    }
}

fn yaml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    quote_into(&mut out, s);
    out.push('"');
    out
}

fn quote_into(out: &mut String, s: &str) {
    let bytes = s.as_bytes();
    let mut start = 0;
    for i in 0..bytes.len() {
        let b = bytes[i];
        if !needs_escape(b) {
            continue;
        }
        out.push_str(&s[start..i]);
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\x{b:02x}");
            }
        }
        start = i + 1;
    }
    out.push_str(&s[start..]);
}

/// Writes an [`Msbt`] as YAML to `w`. Pass a [`Registry`] to name control tags.
pub fn emit_msbt<W: Write>(m: &Msbt, reg: Option<&Registry>, w: &mut W) -> Result<()> {
    emit_header(w, &m.header)?;
    emit_sections(w, &m.order)?;

    let mut raw_sections: Vec<[u8; 4]> = Vec::new();
    for magic in &m.order {
        let meta = m.meta.get(magic).cloned().unwrap_or_default();
        let reserved = hex_encode(&meta.reserved);
        let padding = hex_encode(&[meta.padding]);
        match magic {
            b"LBL1" => writeln!(
                w,
                "  lbl1: {{ reserved: \"{reserved}\", padding: \"{padding}\", buckets: {} }}",
                m.lbl1_buckets
            )?,
            b"ATR1" => {
                if m.atr1_pool.is_empty() {
                    writeln!(
                        w,
                        "  atr1: {{ reserved: \"{reserved}\", padding: \"{padding}\", num_attrs: {}, attr_size: {} }}",
                        m.messages.len(),
                        m.attr_size
                    )?;
                } else {
                    writeln!(
                        w,
                        "  atr1: {{ reserved: \"{reserved}\", padding: \"{padding}\", num_attrs: {}, attr_size: {}, string_data: \"{}\" }}",
                        m.messages.len(),
                        m.attr_size,
                        hex_encode(&m.atr1_pool)
                    )?;
                }
            }
            b"TSY1" => writeln!(
                w,
                "  tsy1: {{ reserved: \"{reserved}\", padding: \"{padding}\" }}"
            )?,
            b"TXT2" => writeln!(
                w,
                "  txt2: {{ reserved: \"{reserved}\", padding: \"{padding}\" }}"
            )?,
            _ => raw_sections.push(*magic),
        }
    }

    if !raw_sections.is_empty() {
        writeln!(w, "  raw_sections:")?;
        for magic in &raw_sections {
            let meta = m.meta.get(magic).cloned().unwrap_or_default();
            let reserved = hex_encode(&meta.reserved);
            let padding = hex_encode(&[meta.padding]);
            let ty = String::from_utf8_lossy(magic);
            if magic == b"ATO1" {
                let ints = m
                    .ato1_ints()
                    .iter()
                    .map(|o| match o {
                        Some(v) => v.to_string(),
                        None => "null".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    w,
                    "    - {{ type: \"{ty}\", reserved: \"{reserved}\", padding: \"{padding}\", ints: [{ints}] }}"
                )?;
            } else {
                let body = m.raw.get(magic).map(|b| hex_encode(b)).unwrap_or_default();
                writeln!(
                    w,
                    "    - {{ type: \"{ty}\", reserved: \"{reserved}\", padding: \"{padding}\", bytes: \"{body}\" }}"
                )?;
            }
        }
    }

    w.write_all(b"messages:\n")?;
    for msg in &m.messages {
        emit_message(w, msg, reg)?;
    }
    Ok(())
}

fn emit_message<W: Write>(
    w: &mut W,
    msg: &crate::formats::msbt::Message,
    reg: Option<&Registry>,
) -> Result<()> {
    w.write_all(b"  - label: ")?;
    write_quoted(w, &msg.label)?;
    writeln!(w, "\n    style: {}", msg.style)?;
    if let Some(r) = reg {
        w.write_all(b"    attrs:\n")?;
        let boundary = r.write_attrs(w, &msg.attr)?;
        w.write_all(b"      attr_trailing: \"")?;
        write_hex(w, &msg.attr[boundary..])?;
        w.write_all(b"\"\n")?;
        let text = r.decode_text(&msg.text);
        emit_text(w, &text)?;
    } else {
        w.write_all(b"    attr_raw: ")?;
        write_quoted(w, &hex_encode(&msg.attr))?;
        w.write_all(b"\n    text_raw: ")?;
        write_quoted(w, &hex_encode(&msg.text))?;
        w.write_all(b"\n")?;
    }
    Ok(())
}

fn emit_text<W: Write>(w: &mut W, text: &str) -> Result<()> {
    if text.is_empty() {
        w.write_all(b"    text: \"\"\n")?;
        return Ok(());
    }
    if is_block_safe(text) {
        w.write_all(b"    text: |-\n")?;
        for line in text.split('\n') {
            w.write_all(b"      ")?;
            w.write_all(line.as_bytes())?;
            w.write_all(b"\n")?;
        }
    } else {
        w.write_all(b"    text: ")?;
        write_quoted(w, text)?;
        w.write_all(b"\n")?;
    }
    Ok(())
}

fn is_block_safe(text: &str) -> bool {
    if text.starts_with('\n') || text.ends_with('\n') || text.ends_with(' ') {
        return false;
    }
    for line in text.split('\n') {
        if line.starts_with(' ') || line.starts_with('\t') {
            return false;
        }
        if line.ends_with(' ') || line.ends_with('\t') {
            return false;
        }
        if line.as_bytes().iter().any(|&b| b < 0x20) {
            return false;
        }
    }
    true
}

/// Writes an [`Msbp`] as YAML to `w`.
pub fn emit_msbp<W: Write>(m: &Msbp, w: &mut W) -> Result<()> {
    emit_header(w, &m.header)?;
    emit_sections(w, &m.order)?;
    for magic in &m.order {
        let meta = m.meta.get(magic).cloned().unwrap_or_default();
        let reserved = hex_encode(&meta.reserved);
        let padding = hex_encode(&[meta.padding]);
        let key = String::from_utf8_lossy(magic).to_ascii_lowercase();
        let mut extra = String::new();
        match magic {
            b"CLB1" | b"ALB1" => extra = format!(", buckets: {}", meta.buckets),
            b"TGG2" | b"TAG2" | b"TGP2" | b"TGL2" => extra = format!(", pad: {}", meta.pad),
            _ => {}
        }
        writeln!(
            w,
            "  {key}: {{ reserved: \"{reserved}\", padding: \"{padding}\"{extra} }}"
        )?;
    }

    writeln!(w, "colors:")?;
    for c in &m.colors {
        let name = c.name.clone().unwrap_or_default();
        writeln!(
            w,
            "  - {{ name: {}, rgba: \"{}\" }}",
            yaml_quote(&name),
            hex_encode(&c.rgba)
        )?;
    }

    writeln!(w, "attributes:")?;
    for a in &m.attributes {
        let name = yaml_quote(a.name.as_deref().unwrap_or(""));
        let ty = msbp::type_name(a.ty);
        if a.ty == 9 {
            writeln!(
                w,
                "  - {{ name: {name}, type: {ty}, list_index: {}, offset: {} }}",
                a.list_index, a.offset
            )?;
        } else {
            writeln!(
                w,
                "  - {{ name: {name}, type: {ty}, offset: {} }}",
                a.offset
            )?;
        }
    }

    writeln!(w, "attribute_lists:")?;
    for (i, list) in m.attribute_lists.iter().enumerate() {
        let items = list
            .iter()
            .map(|s| yaml_quote(s))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(w, "  - {{ name: \"<list {i}>\", items: [{items}] }}")?;
    }

    emit_tag_groups(w, m)?;

    writeln!(w, "tag_params:")?;
    for p in &m.tag_params {
        let name = yaml_quote(&p.name);
        let ty = msbp::type_name(p.ty);
        if p.ty == 9 {
            let li = p
                .list_indices
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                w,
                "  - {{ name: {name}, type: {ty}, list_indices: [{li}] }}"
            )?;
        } else {
            writeln!(w, "  - {{ name: {name}, type: {ty}, pad: {} }}", p.pad)?;
        }
    }

    writeln!(w, "tag_param_lists:")?;
    for (i, s) in m.tag_param_lists.iter().enumerate() {
        writeln!(w, "  - {{ name: \"<list {i}>\", item: {} }}", yaml_quote(s))?;
    }

    writeln!(w, "sources:")?;
    for s in &m.sources {
        writeln!(w, "  - {}", yaml_quote(s))?;
    }
    Ok(())
}

fn emit_tag_groups<W: Write>(w: &mut W, m: &Msbp) -> Result<()> {
    writeln!(w, "tag_groups:")?;
    for g in &m.tag_groups {
        writeln!(w, "  - name: {}", yaml_quote(&g.name))?;
        if g.id != 0 {
            writeln!(w, "    id: {}", g.id)?;
        }
        let idxs = g
            .tag_indices
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(w, "    tag_indices: [{idxs}]")?;
        writeln!(w, "    tags:")?;
        for &gi in &g.tag_indices {
            if let Some(t) = m.tags.get(gi as usize) {
                let pi = t
                    .param_indices
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    w,
                    "      - {{ name: {}, param_indices: [{pi}] }}",
                    yaml_quote(&t.name)
                )?;
            }
        }
    }
    Ok(())
}
