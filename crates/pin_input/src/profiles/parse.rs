//! Profile TOML parser with precise errors (format: see the module docs).

use super::{
    AxisBinding, DeviceMatch, DeviceProfile, GamepadAxis, NudgeDirection, ProfileError, RawInput,
    cabinet_axis_from_name,
};
use pin_core::input::CabinetInput;
use std::ops::Range;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

type SValue<'i> = Spanned<DeValue<'i>>;

/// Parses a profile file.
pub fn parse_profile(toml_text: &str) -> Result<DeviceProfile, ProfileError> {
    let doc = DeTable::parse(toml_text).map_err(|e| {
        let at = e.span().map_or(0, |s| s.start);
        let (line, col) = line_col(toml_text, at);
        ProfileError::Syntax {
            line,
            col,
            message: e.message().trim().to_string(),
        }
    })?;
    Parser { text: toml_text }.document(doc.get_ref())
}

/// 1-based line and column (in characters) of byte offset `at`.
fn line_col(text: &str, at: usize) -> (usize, usize) {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let before = &text[..at];
    let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let col = before[line_start..].chars().count() + 1;
    (line, col)
}

struct Parser<'t> {
    text: &'t str,
}

impl Parser<'_> {
    fn err(
        &self,
        key: &str,
        span: Option<Range<usize>>,
        message: impl Into<String>,
    ) -> ProfileError {
        ProfileError::Invalid {
            key: key.to_string(),
            position: span.map(|s| line_col(self.text, s.start)),
            message: message.into(),
        }
    }

    fn type_err(&self, key: &str, v: &SValue<'_>, want: &str) -> ProfileError {
        self.err(
            key,
            Some(v.span()),
            format!("expected {want}, found {}", type_name(v.get_ref())),
        )
    }

    fn table<'a, 'i>(&self, key: &str, v: &'a SValue<'i>) -> Result<&'a DeTable<'i>, ProfileError> {
        match v.get_ref() {
            DeValue::Table(t) => Ok(t),
            _ => Err(self.type_err(key, v, "a table")),
        }
    }

    fn string<'a>(&self, key: &str, v: &'a SValue<'_>) -> Result<&'a str, ProfileError> {
        match v.get_ref() {
            DeValue::String(s) => Ok(s.as_ref()),
            _ => Err(self.type_err(key, v, "a string")),
        }
    }

    fn boolean(&self, key: &str, v: &SValue<'_>) -> Result<bool, ProfileError> {
        match v.get_ref() {
            DeValue::Boolean(b) => Ok(*b),
            _ => Err(self.type_err(key, v, "a boolean")),
        }
    }

    fn number(&self, key: &str, v: &SValue<'_>) -> Result<f32, ProfileError> {
        let f = match v.get_ref() {
            DeValue::Float(f) => f.as_str().parse::<f64>().ok(),
            DeValue::Integer(i) => i64::from_str_radix(i.as_str(), i.radix())
                .ok()
                .map(|i| i as f64),
            _ => return Err(self.type_err(key, v, "a number")),
        };
        match f {
            Some(f) if f.is_finite() && f.abs() <= 1.0e6 => Ok(f as f32),
            _ => Err(self.err(key, Some(v.span()), "number out of range (|x| <= 1e6)")),
        }
    }

    fn u16_value(&self, key: &str, v: &SValue<'_>) -> Result<u16, ProfileError> {
        match v.get_ref() {
            DeValue::Integer(i) => i64::from_str_radix(i.as_str(), i.radix())
                .ok()
                .and_then(|i| u16::try_from(i).ok())
                .ok_or_else(|| self.err(key, Some(v.span()), "expected an integer in 0..=0xFFFF")),
            _ => Err(self.type_err(key, v, "an integer")),
        }
    }

    /// One raw spec or a list of them.
    fn raw_list(&self, key: &str, v: &SValue<'_>) -> Result<Vec<(RawInput, String)>, ProfileError> {
        let mut out = Vec::new();
        match v.get_ref() {
            DeValue::String(_) => out.push((self.raw(key, v)?, key.to_string())),
            DeValue::Array(a) => {
                for (i, item) in a.iter().enumerate() {
                    let k = format!("{key}[{i}]");
                    out.push((self.raw(&k, item)?, k));
                }
            }
            _ => return Err(self.type_err(key, v, "a raw input string or an array of them")),
        }
        Ok(out)
    }

    fn raw(&self, key: &str, v: &SValue<'_>) -> Result<RawInput, ProfileError> {
        let s = self.string(key, v)?;
        RawInput::parse(s).ok_or_else(|| {
            self.err(
                key,
                Some(v.span()),
                format!(
                    "malformed raw input {s:?} (expected \"key:<scancode>\", \
                     \"button:<gamepad button>\" or \"axis:<gamepad axis>\")"
                ),
            )
        })
    }

    fn document(&self, doc: &DeTable<'_>) -> Result<DeviceProfile, ProfileError> {
        let mut id = None;
        let mut name = None;
        let mut matches = Vec::new();
        let mut buttons = Vec::new();
        let mut axes = Vec::new();
        let mut nudge_buttons = Vec::new();
        for (k, v) in doc.iter() {
            let key = k.get_ref().as_ref();
            match key {
                "id" => {
                    let s = self.string(key, v)?;
                    if s.trim().is_empty() {
                        return Err(self.err(key, Some(v.span()), "must not be empty"));
                    }
                    id = Some(s.to_string());
                }
                "name" => name = Some(self.string(key, v)?.to_string()),
                "match" => matches = self.matches(v)?,
                "buttons" => buttons = self.buttons(v)?,
                "axes" => axes = self.axes(v)?,
                "nudge" => nudge_buttons = self.nudge(v)?,
                _ => {
                    return Err(self.err(
                        key,
                        Some(k.span()),
                        "unknown key (expected id, name, match, buttons, axes or nudge)",
                    ));
                }
            }
        }
        let id = id.ok_or_else(|| self.err("id", None, "missing required key"))?;
        let name = name.ok_or_else(|| self.err("name", None, "missing required key"))?;
        // A raw axis drives either an analog binding or a button, not both.
        for (raw, input) in &buttons {
            if let RawInput::Axis(a) = raw
                && axes.iter().any(|b: &AxisBinding| b.raw == *a)
            {
                return Err(self.err(
                    &format!("buttons.{}", input.name()),
                    None,
                    format!("raw axis {raw} is also bound in [[axes]]"),
                ));
            }
        }
        Ok(DeviceProfile {
            id,
            name,
            matches,
            buttons,
            axes,
            nudge_buttons,
        })
    }

    fn matches(&self, v: &SValue<'_>) -> Result<Vec<DeviceMatch>, ProfileError> {
        let DeValue::Array(a) = v.get_ref() else {
            return Err(self.type_err("match", v, "an array"));
        };
        let mut out = Vec::new();
        for (i, item) in a.iter().enumerate() {
            let key = format!("match[{i}]");
            let t = self.table(&key, item)?;
            let mut entries = t.iter();
            let (Some((mk, mv)), None) = (entries.next(), entries.next()) else {
                return Err(self.err(
                    &key,
                    Some(item.span()),
                    "expected exactly one of gamepad, any_gamepad or keyboard",
                ));
            };
            let mkey = format!("{key}.{}", mk.get_ref());
            let m = match mk.get_ref().as_ref() {
                "keyboard" | "any_gamepad" => {
                    if !self.boolean(&mkey, mv)? {
                        return Err(self.err(&mkey, Some(mv.span()), "must be true"));
                    }
                    if mk.get_ref() == "keyboard" {
                        DeviceMatch::Keyboard
                    } else {
                        DeviceMatch::AnyGamepad
                    }
                }
                "gamepad" => {
                    let g = self.table(&mkey, mv)?;
                    let mut vendor = None;
                    let mut product = None;
                    for (gk, gv) in g.iter() {
                        let gkey = format!("{mkey}.{}", gk.get_ref());
                        match gk.get_ref().as_ref() {
                            "vendor" => vendor = Some(self.u16_value(&gkey, gv)?),
                            "product" => product = Some(self.u16_value(&gkey, gv)?),
                            _ => {
                                return Err(self.err(
                                    &gkey,
                                    Some(gk.span()),
                                    "unknown key (expected vendor or product)",
                                ));
                            }
                        }
                    }
                    let vendor = vendor.ok_or_else(|| {
                        self.err(&format!("{mkey}.vendor"), Some(mv.span()), "missing")
                    })?;
                    let product = product.ok_or_else(|| {
                        self.err(&format!("{mkey}.product"), Some(mv.span()), "missing")
                    })?;
                    DeviceMatch::Gamepad { vendor, product }
                }
                _ => {
                    return Err(self.err(
                        &mkey,
                        Some(mk.span()),
                        "unknown match kind (expected gamepad, any_gamepad or keyboard)",
                    ));
                }
            };
            out.push(m);
        }
        Ok(out)
    }

    fn buttons(&self, v: &SValue<'_>) -> Result<Vec<(RawInput, CabinetInput)>, ProfileError> {
        let t = self.table("buttons", v)?;
        let mut out: Vec<(RawInput, CabinetInput)> = Vec::new();
        for (k, list) in t.iter() {
            let key = format!("buttons.{}", k.get_ref());
            let input = CabinetInput::from_name(k.get_ref()).ok_or_else(|| {
                self.err(
                    &key,
                    Some(k.span()),
                    format!(
                        "unknown cabinet input {:?} (expected left_flipper, right_flipper, \
                         left_upper, right_upper, action, start, launch or tilt)",
                        k.get_ref().as_ref()
                    ),
                )
            })?;
            for (raw, rkey) in self.raw_list(&key, list)? {
                if let Some((_, other)) = out.iter().find(|(r, _)| *r == raw) {
                    return Err(self.err(
                        &rkey,
                        Some(list.span()),
                        format!("raw input {raw} is already bound to {}", other.name()),
                    ));
                }
                out.push((raw, input));
            }
        }
        out.sort_by_key(|(_, c)| c.index());
        Ok(out)
    }

    fn nudge(&self, v: &SValue<'_>) -> Result<Vec<(RawInput, NudgeDirection)>, ProfileError> {
        let t = self.table("nudge", v)?;
        let mut out: Vec<(RawInput, NudgeDirection)> = Vec::new();
        for (k, list) in t.iter() {
            let key = format!("nudge.{}", k.get_ref());
            let dir = NudgeDirection::from_name(k.get_ref()).ok_or_else(|| {
                self.err(
                    &key,
                    Some(k.span()),
                    "unknown nudge direction (expected left, right or forward)",
                )
            })?;
            for (raw, rkey) in self.raw_list(&key, list)? {
                if let Some((_, other)) = out.iter().find(|(r, _)| *r == raw) {
                    return Err(self.err(
                        &rkey,
                        Some(list.span()),
                        format!("raw input {raw} is already bound to nudge {}", other.name()),
                    ));
                }
                out.push((raw, dir));
            }
        }
        out.sort_by_key(|(_, d)| *d as usize);
        Ok(out)
    }

    fn axes(&self, v: &SValue<'_>) -> Result<Vec<AxisBinding>, ProfileError> {
        let DeValue::Array(a) = v.get_ref() else {
            return Err(self.type_err("axes", v, "an array of tables ([[axes]])"));
        };
        let mut out: Vec<AxisBinding> = Vec::new();
        for (i, item) in a.iter().enumerate() {
            let key = format!("axes[{i}]");
            let t = self.table(&key, item)?;
            let mut raw = None;
            let mut axis = None;
            let mut from = None;
            let mut to = None;
            let mut invert = false;
            for (k, fv) in t.iter() {
                let fkey = format!("{key}.{}", k.get_ref());
                match k.get_ref().as_ref() {
                    "raw" => {
                        let s = self.string(&fkey, fv)?;
                        let name = s.strip_prefix("axis:").unwrap_or(s);
                        raw = Some(GamepadAxis::from_name(name).ok_or_else(|| {
                            self.err(
                                &fkey,
                                Some(fv.span()),
                                format!(
                                    "unknown gamepad axis {s:?} (expected left_x, left_y, \
                                     right_x, right_y, left_trigger or right_trigger)"
                                ),
                            )
                        })?);
                    }
                    "axis" => {
                        let s = self.string(&fkey, fv)?;
                        axis = Some(cabinet_axis_from_name(s).ok_or_else(|| {
                            self.err(
                                &fkey,
                                Some(fv.span()),
                                format!(
                                    "unknown cabinet axis {s:?} (expected plunger, nudge_x, \
                                     nudge_y, left_flipper or right_flipper)"
                                ),
                            )
                        })?);
                    }
                    "from" => from = Some((self.number(&fkey, fv)?, fv.span())),
                    "to" => to = Some((self.number(&fkey, fv)?, fv.span())),
                    "invert" => invert = self.boolean(&fkey, fv)?,
                    _ => {
                        return Err(self.err(
                            &fkey,
                            Some(k.span()),
                            "unknown key (expected raw, axis, from, to or invert)",
                        ));
                    }
                }
            }
            let raw = raw.ok_or_else(|| {
                self.err(
                    &format!("{key}.raw"),
                    Some(item.span()),
                    "missing required key",
                )
            })?;
            let axis = axis.ok_or_else(|| {
                self.err(
                    &format!("{key}.axis"),
                    Some(item.span()),
                    "missing required key",
                )
            })?;
            let from_v = from.as_ref().map_or(raw.raw_min(), |f| f.0);
            let to_v = to.as_ref().map_or(1.0, |t| t.0);
            if from_v == to_v {
                let span = to.or(from).map(|s| s.1).unwrap_or(item.span());
                return Err(self.err(
                    &format!("{key}.to"),
                    Some(span),
                    "`from` and `to` must differ",
                ));
            }
            if out.iter().any(|b| b.raw == raw) {
                return Err(self.err(
                    &format!("{key}.raw"),
                    Some(item.span()),
                    format!("raw axis {} is already bound", raw.name()),
                ));
            }
            out.push(AxisBinding {
                raw,
                axis,
                invert,
                from: from_v,
                to: to_v,
            });
        }
        Ok(out)
    }
}

fn type_name(v: &DeValue<'_>) -> &'static str {
    match v {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a datetime",
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_counts_chars() {
        let t = "ab\ncdé f\n";
        assert_eq!(line_col(t, 0), (1, 1));
        assert_eq!(line_col(t, 3), (2, 1));
        // 'f' is after a 2-byte char: byte 8, char column 5.
        assert_eq!(line_col(t, 8), (2, 5));
        // Inside a multi-byte char: rounds down.
        assert_eq!(line_col(t, 6), (2, 3));
        assert_eq!(line_col(t, 1000), (3, 1));
    }
}
