//! Value expressions: numbers with units, parameters and arithmetic, as typed
//! into dimension and feature boxes (`10 mm`, `2in + $d`, `$w / 2`).

use serde::{Deserialize, Serialize};

use crate::units::Unit;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dim {
    /// A bare number; read as document units or degrees where a length or angle is wanted.
    None,
    Length,
    Angle,
}

/// Lengths are in millimetres and angles in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quantity {
    pub v: f64,
    pub dim: Dim,
}

/// What a box expects its expression to produce.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Length,
    Angle,
    Scalar,
}

/// An expression with its last evaluated result (mm, degrees or a plain number).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Value {
    pub expr: String,
    #[serde(default)]
    pub v: f64,
}

impl Value {
    /// Whether the expression is more than a single number with a unit.
    pub fn is_formula(&self) -> bool {
        self.expr.chars().any(|c| "$+*/()".contains(c))
            || self
                .expr
                .trim_start()
                .starts_with(|c: char| c.is_alphabetic())
    }
}

pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<Result<Quantity, String>>;

const UNITS: [(&str, Dim, f64); 9] = [
    ("mm", Dim::Length, 1.0),
    ("cm", Dim::Length, 10.0),
    ("m", Dim::Length, 1000.0),
    ("in", Dim::Length, 25.4),
    ("inch", Dim::Length, 25.4),
    ("ft", Dim::Length, 304.8),
    ("deg", Dim::Angle, 1.0),
    ("rad", Dim::Angle, 57.29577951308232),
    ("pi", Dim::None, 0.0),
];

fn unit_of(name: &str) -> Option<(Dim, f64)> {
    UNITS
        .iter()
        .find(|u| u.0 == name && u.0 != "pi")
        .map(|u| (u.1, u.2))
}

/// Whether `name` can be used for a parameter.
pub fn valid_name(name: &str) -> bool {
    let mut c = name.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !UNITS.iter().any(|u| u.0 == name)
        && !FUNCS.contains(&name)
}

const FUNCS: [&str; 5] = ["sqrt", "abs", "sin", "cos", "tan"];

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Var(String),
    Op(char),
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    if s.len() > 4096 {
        return Err("the expression is too long (maximum 4096 bytes)".into());
    }
    let ch: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < ch.len() {
        let c = ch[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let st = i;
            while i < ch.len() && (ch[i].is_ascii_digit() || ch[i] == '.') {
                i += 1;
            }
            let t: String = ch[st..i].iter().collect();
            out.push(Tok::Num(
                t.parse().map_err(|_| format!("bad number '{t}'"))?,
            ));
        } else if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let st = if c == '$' { i + 1 } else { i };
            i = st;
            while i < ch.len() && (ch[i].is_ascii_alphanumeric() || ch[i] == '_') {
                i += 1;
            }
            let t: String = ch[st..i].iter().collect();
            if t.is_empty() {
                return Err("expected a name after '$'".into());
            }
            out.push(if c == '$' { Tok::Var(t) } else { Tok::Ident(t) });
        } else if c == '"' || c == '\u{2033}' {
            out.push(Tok::Ident("in".into()));
            i += 1;
        } else if c == '\u{b0}' {
            out.push(Tok::Ident("deg".into()));
            i += 1;
        } else if "+-*/()".contains(c) {
            out.push(Tok::Op(c));
            i += 1;
        } else {
            return Err(format!("unexpected '{c}'"));
        }
    }
    Ok(out)
}

struct Parser<'a> {
    t: Vec<Tok>,
    i: usize,
    unit: Unit,
    look: Lookup<'a>,
    depth: usize,
    /// Token ranges implicitly promoted to a length during addition/subtraction.
    lengths: Vec<(usize, usize)>,
}

impl Parser<'_> {
    fn eat(&mut self, c: char) -> bool {
        let hit = self.t.get(self.i) == Some(&Tok::Op(c));
        if hit {
            self.i += 1;
        }
        hit
    }

    /// Brings two operands of `+`/`-` to a common dimension.
    fn unify(&self, a: Quantity, b: Quantity) -> Result<(f64, f64, Dim), String> {
        match (a.dim, b.dim) {
            (x, y) if x == y => Ok((a.v, b.v, x)),
            (Dim::None, Dim::Length) => Ok((a.v * self.unit.mm(), b.v, Dim::Length)),
            (Dim::Length, Dim::None) => Ok((a.v, b.v * self.unit.mm(), Dim::Length)),
            (Dim::None, Dim::Angle) | (Dim::Angle, Dim::None) => Ok((a.v, b.v, Dim::Angle)),
            _ => Err("cannot add a length and an angle".into()),
        }
    }

    fn expr(&mut self) -> Result<Quantity, String> {
        let start = self.i;
        let mut a = self.term()?;
        loop {
            let left_end = self.i;
            let sign = if self.eat('+') {
                1.0
            } else if self.eat('-') {
                -1.0
            } else {
                return Ok(a);
            };
            let right_start = self.i;
            let b = self.term()?;
            match (a.dim, b.dim) {
                (Dim::None, Dim::Length) => self.lengths.push((start, left_end)),
                (Dim::Length, Dim::None) => self.lengths.push((right_start, self.i)),
                _ => {}
            }
            let (x, y, dim) = self.unify(a, b)?;
            a = Quantity {
                v: x + sign * y,
                dim,
            };
        }
    }

    fn term(&mut self) -> Result<Quantity, String> {
        let mut a = self.unary()?;
        loop {
            if self.eat('*') {
                let b = self.unary()?;
                let dim = match (a.dim, b.dim) {
                    (Dim::None, d) | (d, Dim::None) => d,
                    _ => return Err("cannot multiply two values that both have units".into()),
                };
                a = Quantity { v: a.v * b.v, dim };
            } else if self.eat('/') {
                let b = self.unary()?;
                if b.v == 0.0 {
                    return Err("division by zero".into());
                }
                let dim = match (a.dim, b.dim) {
                    (d, Dim::None) => d,
                    (x, y) if x == y => Dim::None,
                    _ => return Err("cannot divide these units".into()),
                };
                a = Quantity { v: a.v / b.v, dim };
            } else {
                return Ok(a);
            }
        }
    }

    fn unary(&mut self) -> Result<Quantity, String> {
        if self.depth >= 32 {
            return Err("the expression is nested too deeply (maximum 32 levels)".into());
        }
        self.depth += 1;
        let result = self.unary_inner();
        self.depth -= 1;
        result
    }

    fn unary_inner(&mut self) -> Result<Quantity, String> {
        if self.eat('-') {
            let q = self.unary()?;
            return Ok(Quantity {
                v: -q.v,
                dim: q.dim,
            });
        }
        if self.eat('+') {
            return self.unary();
        }
        let mut q = self.primary()?;
        // A unit name directly after a value applies to it: `10 mm`, `($a + 2) in`.
        if let Some(Tok::Ident(name)) = self.t.get(self.i)
            && let Some((dim, f)) = unit_of(name)
        {
            if q.dim != Dim::None {
                return Err(format!("'{name}' follows a value that already has a unit"));
            }
            q = Quantity { v: q.v * f, dim };
            self.i += 1;
        }
        Ok(q)
    }

    fn primary(&mut self) -> Result<Quantity, String> {
        let tok = self
            .t
            .get(self.i)
            .cloned()
            .ok_or("the expression is incomplete")?;
        self.i += 1;
        match tok {
            Tok::Num(v) => Ok(Quantity { v, dim: Dim::None }),
            Tok::Op('(') => {
                let q = self.expr()?;
                if !self.eat(')') {
                    return Err("missing ')'".into());
                }
                Ok(q)
            }
            Tok::Var(name) => {
                (self.look)(&name).unwrap_or_else(|| Err(format!("no parameter named '{name}'")))
            }
            Tok::Ident(name) => {
                if FUNCS.contains(&name.as_str()) {
                    if !self.eat('(') {
                        return Err(format!("{name} needs parentheses"));
                    }
                    let q = self.expr()?;
                    if !self.eat(')') {
                        return Err("missing ')'".into());
                    }
                    let plain = |v: f64| Ok(Quantity { v, dim: Dim::None });
                    return match name.as_str() {
                        "abs" => Ok(Quantity {
                            v: q.v.abs(),
                            dim: q.dim,
                        }),
                        "sqrt" if q.dim == Dim::None && q.v >= 0.0 => plain(q.v.sqrt()),
                        "sqrt" => Err("sqrt needs a plain non-negative number".into()),
                        _ if q.dim == Dim::Length => Err(format!("{name} needs an angle")),
                        "sin" => plain(q.v.to_radians().sin()),
                        "cos" => plain(q.v.to_radians().cos()),
                        _ => plain(q.v.to_radians().tan()),
                    };
                }
                if let Some(r) = (self.look)(&name) {
                    return r;
                }
                if name == "pi" {
                    return Ok(Quantity {
                        v: std::f64::consts::PI,
                        dim: Dim::None,
                    });
                }
                Err(if unit_of(&name).is_some() {
                    format!("'{name}' needs a number before it")
                } else {
                    format!("no parameter named '{name}'")
                })
            }
            Tok::Op(c) => Err(format!("unexpected '{c}'")),
        }
    }
}

/// Evaluates `src`; bare numbers mixed with lengths are read in `unit`.
pub fn eval(src: &str, unit: Unit, look: Lookup) -> Result<Quantity, String> {
    evaluate(src, unit, look, false).map(|(q, _)| q)
}

/// Makes implicit length operands explicit while retaining parameter references.
/// Storing this text keeps `width + 2` at its original size after changing units.
pub fn eval_pinned(src: &str, unit: Unit, look: Lookup) -> Result<(Quantity, String), String> {
    evaluate(src, unit, look, true)
}

fn evaluate(src: &str, unit: Unit, look: Lookup, pin: bool) -> Result<(Quantity, String), String> {
    let t = lex(src)?;
    if t.is_empty() {
        return Err("enter a value".into());
    }
    let mut p = Parser {
        t,
        i: 0,
        unit,
        look,
        depth: 0,
        lengths: Vec::new(),
    };
    let q = p.expr()?;
    if p.i < p.t.len() {
        return Err("unexpected text after the value".into());
    }
    if !q.v.is_finite() {
        return Err("the value is not finite".into());
    }
    let text = if !pin || p.lengths.is_empty() {
        src.trim().to_owned()
    } else {
        // Render the original tokens with units around precisely the operands
        // promoted by the evaluator. Multipliers and divisors stay scalars.
        let mut out = Vec::new();
        for (i, token) in p.t.iter().enumerate() {
            out.extend(
                p.lengths
                    .iter()
                    .filter(|(start, _)| *start == i)
                    .map(|_| "(".to_owned()),
            );
            out.push(match token {
                Tok::Num(v) => v.to_string(),
                Tok::Ident(name) => name.clone(),
                Tok::Var(name) => format!("${name}"),
                Tok::Op(op) => op.to_string(),
            });
            out.extend(
                p.lengths
                    .iter()
                    .filter(|(_, end)| *end == i + 1)
                    .map(|_| format!(") {}", unit.name())),
            );
        }
        out.join(" ")
    };
    if pin && text != src.trim() {
        // Generated text must remain within the same parser limits as input.
        eval(&text, unit, look)?;
    }
    Ok((q, text))
}

/// Converts a result to what a box wants: millimetres, degrees or a plain number.
pub fn to_kind(q: Quantity, kind: Kind, unit: Unit) -> Result<f64, String> {
    let result: Result<f64, String> = match (kind, q.dim) {
        (Kind::Length, Dim::None) => Ok(q.v * unit.mm()),
        (Kind::Length, Dim::Length)
        | (Kind::Angle, Dim::None | Dim::Angle)
        | (Kind::Scalar, Dim::None) => Ok(q.v),
        (Kind::Length, _) => Err("expected a length, not an angle".into()),
        (Kind::Angle, _) => Err("expected an angle, not a length".into()),
        (Kind::Scalar, _) => Err("expected a plain number without units".into()),
    };
    let value = result?;
    if !value.is_finite() {
        return Err("the value is not finite in the requested units".into());
    }
    Ok(value)
}

/// Rewrites a unit-less expression so it keeps its meaning if the document
/// units change later: `10` becomes `10 mm`, `$n * 2` becomes `($n * 2) mm`.
pub fn pin_unit(src: &str, q: Quantity, kind: Kind, unit: Unit) -> String {
    let src = src.trim();
    let suffix = match (kind, q.dim) {
        (Kind::Length, Dim::None) => unit.name(),
        (Kind::Angle, Dim::None) => "deg",
        _ => return src.to_owned(),
    };
    match src.parse::<f64>() {
        Ok(_) => format!("{src} {suffix}"),
        Err(_) => format!("({src}) {suffix}"),
    }
}
