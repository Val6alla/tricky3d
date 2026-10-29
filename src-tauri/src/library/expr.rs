//! The formula of Function surface: z = f(x, y) as the user types it. Numbers, x and y (mm), r (the
//! distance from the middle), pi and e; + - * / ^ (power, right to left) and unary minus;
//! parentheses; and the functions below. Parsed once into a tree, then evaluated per grid point.

/// A parsed formula.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expr {
    Number(f64),
    X,
    Y,
    R,
    Neg(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Func {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sinh,
    Cosh,
    Tanh,
    Sqrt,
    Abs,
    Exp,
    Ln,
    Log10,
    Floor,
    Ceil,
    Round,
    Min,
    Max,
    Pow,
    Sign,
    Mod,
}

const FUNCS: [(&str, Func, usize); 24] = [
    ("sin", Func::Sin, 1),
    ("cos", Func::Cos, 1),
    ("tan", Func::Tan, 1),
    ("asin", Func::Asin, 1),
    ("acos", Func::Acos, 1),
    ("atan", Func::Atan, 1),
    ("atan2", Func::Atan2, 2),
    ("sinh", Func::Sinh, 1),
    ("cosh", Func::Cosh, 1),
    ("tanh", Func::Tanh, 1),
    ("sqrt", Func::Sqrt, 1),
    ("abs", Func::Abs, 1),
    ("exp", Func::Exp, 1),
    ("ln", Func::Ln, 1),
    ("log", Func::Ln, 1),
    ("log10", Func::Log10, 1),
    ("floor", Func::Floor, 1),
    ("ceil", Func::Ceil, 1),
    ("round", Func::Round, 1),
    ("min", Func::Min, 2),
    ("max", Func::Max, 2),
    ("pow", Func::Pow, 2),
    ("sign", Func::Sign, 1),
    ("mod", Func::Mod, 2),
];

/// The function names, for the message about an unknown one.
pub(crate) fn function_names() -> String {
    let names: Vec<&str> = FUNCS.iter().map(|f| f.0).collect();
    names.join(", ")
}

struct Parser<'a> {
    text: &'a [u8],
    pos: usize,
}

/// Parses a formula, or says (in a sentence) what is wrong and where.
pub(crate) fn parse(text: &str) -> Result<Expr, String> {
    if !text.is_ascii() {
        return Err("The formula may only use letters, digits and + - * / ^ ( ) , .".into());
    }
    let mut p = Parser {
        text: text.as_bytes(),
        pos: 0,
    };
    p.skip();
    if p.pos == p.text.len() {
        return Err("Type a formula for z, such as sin(x / 5) * 3.".into());
    }
    let e = p.sum()?;
    p.skip();
    if p.pos < p.text.len() {
        return Err(p.unexpected());
    }
    Ok(e)
}

impl Parser<'_> {
    fn skip(&mut self) {
        while self.pos < self.text.len() && self.text[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.text.get(self.pos).copied()
    }

    fn unexpected(&self) -> String {
        match self.text.get(self.pos) {
            Some(&c) => format!(
                "The formula has \"{}\" where it cannot go (character {}).",
                c as char,
                self.pos + 1
            ),
            None => "The formula ends too early.".into(),
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.unexpected())
        }
    }

    fn sum(&mut self) -> Result<Expr, String> {
        let mut left = self.product()?;
        loop {
            let op = match self.peek() {
                Some(b'+') => Op::Add,
                Some(b'-') => Op::Sub,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.product()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn product(&mut self) -> Result<Expr, String> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(b'*') => Op::Mul,
                Some(b'/') => Op::Div,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(b'-') => {
                self.pos += 1;
                Ok(Expr::Neg(Box::new(self.unary()?)))
            }
            Some(b'+') => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<Expr, String> {
        let base = self.atom()?;
        if self.peek() == Some(b'^') {
            self.pos += 1;
            let exponent = self.unary()?;
            return Ok(Expr::Binary(Op::Pow, Box::new(base), Box::new(exponent)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(c) if c.is_ascii_digit() || c == b'.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() => self.name(),
            Some(b'(') => {
                self.pos += 1;
                let e = self.sum()?;
                self.expect(b')')?;
                Ok(e)
            }
            _ => Err(self.unexpected()),
        }
    }

    fn number(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        while self.pos < self.text.len()
            && (self.text[self.pos].is_ascii_digit() || self.text[self.pos] == b'.')
        {
            self.pos += 1;
        }
        // An exponent: 1e-3, 2.5E4.
        if self.pos < self.text.len() && matches!(self.text[self.pos], b'e' | b'E') {
            let mark = self.pos;
            self.pos += 1;
            if self.pos < self.text.len() && matches!(self.text[self.pos], b'+' | b'-') {
                self.pos += 1;
            }
            if self.pos < self.text.len() && self.text[self.pos].is_ascii_digit() {
                while self.pos < self.text.len() && self.text[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
            } else {
                self.pos = mark;
            }
        }
        let word = std::str::from_utf8(&self.text[start..self.pos]).unwrap_or("");
        word.parse::<f64>()
            .map(Expr::Number)
            .map_err(|_| format!("\"{word}\" is not a number (character {}).", start + 1))
    }

    fn name(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        while self.pos < self.text.len() && self.text[self.pos].is_ascii_alphanumeric() {
            self.pos += 1;
        }
        let word = std::str::from_utf8(&self.text[start..self.pos])
            .unwrap_or("")
            .to_ascii_lowercase();
        match word.as_str() {
            "x" => return Ok(Expr::X),
            "y" => return Ok(Expr::Y),
            "r" => return Ok(Expr::R),
            "pi" => return Ok(Expr::Number(std::f64::consts::PI)),
            "e" => return Ok(Expr::Number(std::f64::consts::E)),
            _ => {}
        }
        let Some(&(_, func, arity)) = FUNCS.iter().find(|f| f.0 == word) else {
            return Err(format!(
                "The formula does not know \"{word}\". Use x, y, r, pi, e and {}.",
                function_names()
            ));
        };
        self.expect(b'(')?;
        let mut args = vec![self.sum()?];
        while self.peek() == Some(b',') {
            self.pos += 1;
            args.push(self.sum()?);
        }
        self.expect(b')')?;
        if args.len() != arity {
            return Err(format!(
                "{word} takes {arity} value{}, not {}.",
                if arity == 1 { "" } else { "s" },
                args.len()
            ));
        }
        Ok(Expr::Call(func, args))
    }
}

impl Expr {
    /// The value at (x, y); NaN or infinite where the formula has none.
    pub(crate) fn eval(&self, x: f64, y: f64) -> f64 {
        match self {
            Expr::Number(v) => *v,
            Expr::X => x,
            Expr::Y => y,
            Expr::R => x.hypot(y),
            Expr::Neg(e) => -e.eval(x, y),
            Expr::Binary(op, a, b) => {
                let (a, b) = (a.eval(x, y), b.eval(x, y));
                match op {
                    Op::Add => a + b,
                    Op::Sub => a - b,
                    Op::Mul => a * b,
                    Op::Div => a / b,
                    Op::Pow => a.powf(b),
                }
            }
            Expr::Call(f, args) => {
                let a = args[0].eval(x, y);
                let b = || args[1].eval(x, y);
                match f {
                    Func::Sin => a.sin(),
                    Func::Cos => a.cos(),
                    Func::Tan => a.tan(),
                    Func::Asin => a.asin(),
                    Func::Acos => a.acos(),
                    Func::Atan => a.atan(),
                    Func::Atan2 => a.atan2(b()),
                    Func::Sinh => a.sinh(),
                    Func::Cosh => a.cosh(),
                    Func::Tanh => a.tanh(),
                    Func::Sqrt => a.sqrt(),
                    Func::Abs => a.abs(),
                    Func::Exp => a.exp(),
                    Func::Ln => a.ln(),
                    Func::Log10 => a.log10(),
                    Func::Floor => a.floor(),
                    Func::Ceil => a.ceil(),
                    Func::Round => a.round(),
                    Func::Min => a.min(b()),
                    Func::Max => a.max(b()),
                    Func::Pow => a.powf(b()),
                    Func::Sign => {
                        if a > 0.0 {
                            1.0
                        } else if a < 0.0 {
                            -1.0
                        } else {
                            0.0
                        }
                    }
                    Func::Mod => a.rem_euclid(b()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, x: f64, y: f64) -> f64 {
        parse(text)
            .unwrap_or_else(|e| panic!("{text}: {e}"))
            .eval(x, y)
    }

    #[test]
    fn precedence_and_associativity_follow_school_rules() {
        assert_eq!(at("1 + 2 * 3", 0.0, 0.0), 7.0);
        assert_eq!(at("(1 + 2) * 3", 0.0, 0.0), 9.0);
        assert_eq!(at("2 ^ 3 ^ 2", 0.0, 0.0), 512.0);
        assert_eq!(at("-2 ^ 2", 0.0, 0.0), -4.0);
        assert_eq!(at("2 ^ -1", 0.0, 0.0), 0.5);
        assert_eq!(at("10 / 4 / 5", 0.0, 0.0), 0.5);
        assert_eq!(at("8 - 3 - 2", 0.0, 0.0), 3.0);
        assert_eq!(at("1.5e1 + .5", 0.0, 0.0), 15.5);
    }

    #[test]
    fn variables_constants_and_functions() {
        assert_eq!(at("x * y", 3.0, 4.0), 12.0);
        assert_eq!(at("r", 3.0, 4.0), 5.0);
        assert!((at("sin(pi / 2)", 0.0, 0.0) - 1.0).abs() < 1e-15);
        assert_eq!(at("max(x, y) - min(x, y)", 2.0, 7.0), 5.0);
        assert_eq!(at("mod(-1, 3)", 0.0, 0.0), 2.0);
        assert_eq!(at("SQRT(X^2 + Y^2)", 6.0, 8.0), 10.0);
        assert_eq!(at("atan2(1, 1) * 4", 0.0, 0.0), std::f64::consts::PI);
        assert!(at("ln(x)", -1.0, 0.0).is_nan());
    }

    #[test]
    fn mistakes_are_named() {
        assert_eq!(parse("2 * ").unwrap_err(), "The formula ends too early.");
        assert_eq!(parse("sin(x").unwrap_err(), "The formula ends too early.");
        assert_eq!(
            parse("x $ y").unwrap_err(),
            "The formula has \"$\" where it cannot go (character 3)."
        );
        assert!(
            parse("foo(x)")
                .unwrap_err()
                .starts_with("The formula does not know \"foo\"")
        );
        assert_eq!(parse("pow(x)").unwrap_err(), "pow takes 2 values, not 1.");
        assert_eq!(
            parse("   ").unwrap_err(),
            "Type a formula for z, such as sin(x / 5) * 3."
        );
        assert!(parse("x × y").is_err());
    }
}
