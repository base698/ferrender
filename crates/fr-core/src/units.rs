//! Display units. Lengths are stored in millimetres everywhere.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Mm,
    Cm,
    In,
}

impl Unit {
    pub const ALL: [Unit; 3] = [Unit::Mm, Unit::Cm, Unit::In];

    /// Millimetres in one of this unit.
    pub fn mm(self) -> f64 {
        match self {
            Unit::Mm => 1.0,
            Unit::Cm => 10.0,
            Unit::In => 25.4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::In => "in",
        }
    }

    pub fn parse(s: &str) -> Option<Unit> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mm" | "millimeter" | "millimeters" => Some(Unit::Mm),
            "cm" | "centimeter" | "centimeters" => Some(Unit::Cm),
            "in" | "inch" | "inches" | "\"" => Some(Unit::In),
            _ => None,
        }
    }

    /// A length in millimetres as a number in this unit.
    pub fn from_mm(self, mm: f64) -> f64 {
        mm / self.mm()
    }
}

/// A number with trailing zeros trimmed, at most `decimals` places.
pub fn trim_num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

/// A length in millimetres shown in `unit`, without the unit name.
pub fn fmt_len(mm: f64, unit: Unit) -> String {
    trim_num(unit.from_mm(mm), if unit == Unit::Mm { 3 } else { 4 })
}
