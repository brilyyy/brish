#![allow(clippy::manual_range_contains)]

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithError {
    BadExpr,
    BadNumber,
    DivZero,
    Depth,
    Overflow,
    BadAssign,
}

impl fmt::Display for ArithError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadExpr => write!(f, "bad expression"),
            Self::BadNumber => write!(f, "bad number"),
            Self::DivZero => write!(f, "division by zero"),
            Self::Depth => write!(f, "expression too deep"),
            Self::Overflow => write!(f, "arithmetic overflow"),
            Self::BadAssign => write!(f, "bad assignment"),
        }
    }
}

impl std::error::Error for ArithError {}

pub trait ArithEnv {
    fn get(&mut self, name: &str) -> Option<i64>;
    fn set(&mut self, name: &str, value: i64);
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            depth: 0,
        }
    }
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }
    fn starts_with(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s)
    }
    fn consume_str(&mut self, s: &str) -> bool {
        if self.starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }
    fn enter(&mut self) -> Result<(), ArithError> {
        self.depth += 1;
        if self.depth > 128 {
            return Err(ArithError::Depth);
        }
        Ok(())
    }
    fn leave(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
        }
    }
    fn parse_comma(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut val = match self.parse_assign(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b',' {
                self.pos += 1;
                let rhs = match self.parse_assign(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                val = rhs;
            } else {
                break;
            }
        }
        self.leave();
        Ok(val)
    }
    fn parse_assign(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        // lookahead: identifier followed by assignment op
        let save = self.pos;
        self.skip_ws();
        let ident = self.peek_ident();
        if let Some(name) = ident {
            // peek assign op without consuming ident first? we already have name, need to see op after it
            // pos is at start of ident, advance past ident temporarily
            let after_ident = self.pos + name.len();
            let rest = &self.src[after_ident..];
            let trimmed = rest.trim_start_matches([' ', '\t', '\n', '\r']);
            let mut op_found: Option<&str> = None;
            for op in [
                "<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "&=", "^=", "|=", "=",
            ] {
                if trimmed.starts_with(op) {
                    if op == "=" && trimmed.starts_with("==") {
                        op_found = None;
                        break;
                    }
                    op_found = Some(op);
                    break;
                }
            }
            if let Some(op) = op_found {
                // consume ident
                self.pos += name.len();
                self.skip_ws();
                // consume op
                let consumed = self.consume_str(op);
                if !consumed {
                    self.leave();
                    return Err(ArithError::BadExpr);
                }
                // parse RHS right-assoc
                let rhs = match self.parse_assign(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                let owned_name = name.to_string();
                let lhs_val = env.get(&owned_name).unwrap_or(0);
                let new_val: Result<i64, ArithError> = match op {
                    "=" => Ok(rhs),
                    "+=" => lhs_val.checked_add(rhs).ok_or(ArithError::Overflow),
                    "-=" => lhs_val.checked_sub(rhs).ok_or(ArithError::Overflow),
                    "*=" => lhs_val.checked_mul(rhs).ok_or(ArithError::Overflow),
                    "/=" => {
                        if rhs == 0 {
                            Err(ArithError::DivZero)
                        } else {
                            lhs_val.checked_div(rhs).ok_or(ArithError::Overflow)
                        }
                    }
                    "%=" => {
                        if rhs == 0 {
                            Err(ArithError::DivZero)
                        } else {
                            lhs_val.checked_rem(rhs).ok_or(ArithError::Overflow)
                        }
                    }
                    "<<=" => {
                        if rhs < 0 || rhs >= 64 {
                            Err(ArithError::Overflow)
                        } else {
                            lhs_val.checked_shl(rhs as u32).ok_or(ArithError::Overflow)
                        }
                    }
                    ">>=" => {
                        if rhs < 0 || rhs >= 64 {
                            Err(ArithError::Overflow)
                        } else {
                            Ok(lhs_val >> rhs)
                        }
                    }
                    "&=" => Ok(lhs_val & rhs),
                    "^=" => Ok(lhs_val ^ rhs),
                    "|=" => Ok(lhs_val | rhs),
                    _ => Err(ArithError::BadExpr),
                };
                let new_val = match new_val {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                env.set(&owned_name, new_val);
                self.leave();
                Ok(new_val)
            } else {
                // not assignment, restore and fall through
                self.pos = save;
                let v = self.parse_ternary(env);
                self.leave();
                v
            }
        } else {
            self.pos = save;
            let v = self.parse_ternary(env);
            self.leave();
            v
        }
    }
    fn parse_ternary(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let cond = match self.parse_logical_or(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        self.skip_ws();
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'?' {
            self.pos += 1;
            let then_val = match self.parse_assign(env) {
                Ok(v) => v,
                Err(e) => {
                    self.leave();
                    return Err(e);
                }
            };
            self.skip_ws();
            if self.pos >= self.bytes.len() || self.bytes[self.pos] != b':' {
                self.leave();
                return Err(ArithError::BadExpr);
            }
            self.pos += 1;
            let else_val = match self.parse_ternary(env) {
                Ok(v) => v,
                Err(e) => {
                    self.leave();
                    return Err(e);
                }
            };
            self.leave();
            if cond != 0 {
                Ok(then_val)
            } else {
                Ok(else_val)
            }
        } else {
            self.leave();
            Ok(cond)
        }
    }
    fn parse_logical_or(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_logical_and(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            if self.starts_with("||") {
                self.pos += 2;
                let rhs = match self.parse_logical_and(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                lhs = if lhs != 0 || rhs != 0 { 1 } else { 0 };
            } else {
                break;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_logical_and(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_bitwise_or(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            if self.starts_with("&&") {
                self.pos += 2;
                let rhs = match self.parse_bitwise_or(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                lhs = if lhs != 0 && rhs != 0 { 1 } else { 0 };
            } else {
                break;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_bitwise_or(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_bitwise_xor(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            if self.starts_with("||") || self.starts_with("&&") {
                break;
            }
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'|' {
                // ensure not "||" (already checked) and not "|="
                if self.starts_with("|=") {
                    break;
                }
                self.pos += 1;
                let rhs = match self.parse_bitwise_xor(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                lhs |= rhs;
            } else {
                break;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_bitwise_xor(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_bitwise_and(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            if self.starts_with("^=") {
                break;
            }
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'^' {
                self.pos += 1;
                let rhs = match self.parse_bitwise_and(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                lhs ^= rhs;
            } else {
                break;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_bitwise_and(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_equality(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            if self.starts_with("&&") || self.starts_with("&=") {
                break;
            }
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'&' {
                self.pos += 1;
                let rhs = match self.parse_equality(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                lhs &= rhs;
            } else {
                break;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_equality(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_relational(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            if self.starts_with("==") {
                self.pos += 2;
                let rhs = match self.parse_relational(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                lhs = if lhs == rhs { 1 } else { 0 };
            } else if self.starts_with("!=") {
                self.pos += 2;
                let rhs = match self.parse_relational(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                lhs = if lhs != rhs { 1 } else { 0 };
            } else {
                break;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_relational(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_shift(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            let op = if self.starts_with("<=") {
                "<="
            } else if self.starts_with(">=") {
                ">="
            } else if self.pos < self.bytes.len()
                && self.bytes[self.pos] == b'<'
                && !self.starts_with("<<")
            {
                "<"
            } else if self.pos < self.bytes.len()
                && self.bytes[self.pos] == b'>'
                && !self.starts_with(">>")
            {
                ">"
            } else {
                ""
            };
            if op.is_empty() {
                break;
            }
            // avoid consuming "<" when it's "<<=" etc. but we handled shift earlier
            if op == "<" || op == ">" || op == "<=" || op == ">=" {
                // ensure not assignment like "<=" is relational, but we must not consume if next is "="
                // actually "<=" is relational, fine
                self.pos += op.len();
                let rhs = match self.parse_shift(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                lhs = match op {
                    "<" => {
                        if lhs < rhs {
                            1
                        } else {
                            0
                        }
                    }
                    "<=" => {
                        if lhs <= rhs {
                            1
                        } else {
                            0
                        }
                    }
                    ">" => {
                        if lhs > rhs {
                            1
                        } else {
                            0
                        }
                    }
                    ">=" => {
                        if lhs >= rhs {
                            1
                        } else {
                            0
                        }
                    }
                    _ => lhs,
                };
            } else {
                break;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_shift(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_add(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            let op = if self.starts_with("<<=") || self.starts_with(">>=") {
                ""
            } else if self.starts_with("<<") {
                "<<"
            } else if self.starts_with(">>") {
                ">>"
            } else {
                ""
            };
            if op.is_empty() {
                break;
            }
            self.pos += 2;
            let rhs = match self.parse_add(env) {
                Ok(v) => v,
                Err(e) => {
                    self.leave();
                    return Err(e);
                }
            };
            if rhs < 0 || rhs >= 64 {
                self.leave();
                return Err(ArithError::Overflow);
            }
            if op == "<<" {
                lhs = match lhs.checked_shl(rhs as u32) {
                    Some(v) => v,
                    None => {
                        self.leave();
                        return Err(ArithError::Overflow);
                    }
                };
            } else {
                // arithmetic right shift
                lhs >>= rhs;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_add(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_mul(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            // need to avoid "++" "--" "+=" "-="
            if self.starts_with("++")
                || self.starts_with("--")
                || self.starts_with("+=")
                || self.starts_with("-=")
            {
                break;
            }
            if self.pos < self.bytes.len()
                && (self.bytes[self.pos] == b'+' || self.bytes[self.pos] == b'-')
            {
                let op = self.bytes[self.pos];
                self.pos += 1;
                let rhs = match self.parse_mul(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                if op == b'+' {
                    lhs = match lhs.checked_add(rhs) {
                        Some(v) => v,
                        None => {
                            self.leave();
                            return Err(ArithError::Overflow);
                        }
                    };
                } else {
                    lhs = match lhs.checked_sub(rhs) {
                        Some(v) => v,
                        None => {
                            self.leave();
                            return Err(ArithError::Overflow);
                        }
                    };
                }
            } else {
                break;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_mul(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let mut lhs = match self.parse_unary(env) {
            Ok(v) => v,
            Err(e) => {
                self.leave();
                return Err(e);
            }
        };
        loop {
            self.skip_ws();
            if self.starts_with("*=") || self.starts_with("/=") || self.starts_with("%=") {
                break;
            }
            if self.pos < self.bytes.len()
                && (self.bytes[self.pos] == b'*'
                    || self.bytes[self.pos] == b'/'
                    || self.bytes[self.pos] == b'%')
            {
                // need to distinguish "*" vs "**" - second * is mul again
                let op = self.bytes[self.pos];
                self.pos += 1;
                let rhs = match self.parse_unary(env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.leave();
                        return Err(e);
                    }
                };
                match op {
                    b'*' => {
                        lhs = match lhs.checked_mul(rhs) {
                            Some(v) => v,
                            None => {
                                self.leave();
                                return Err(ArithError::Overflow);
                            }
                        };
                    }
                    b'/' => {
                        if rhs == 0 {
                            self.leave();
                            return Err(ArithError::DivZero);
                        }
                        lhs = match lhs.checked_div(rhs) {
                            Some(v) => v,
                            None => {
                                self.leave();
                                return Err(ArithError::Overflow);
                            }
                        };
                    }
                    b'%' => {
                        if rhs == 0 {
                            self.leave();
                            return Err(ArithError::DivZero);
                        }
                        lhs = match lhs.checked_rem(rhs) {
                            Some(v) => v,
                            None => {
                                self.leave();
                                return Err(ArithError::Overflow);
                            }
                        };
                    }
                    _ => {}
                }
            } else {
                break;
            }
        }
        self.leave();
        Ok(lhs)
    }
    fn parse_unary(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.enter()?;
        let result = self.parse_unary_inner(env);
        self.leave();
        result
    }
    fn parse_unary_inner(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.skip_ws();
        if self.pos >= self.bytes.len() {
            return Err(ArithError::BadExpr);
        }
        // prefix ++ / --
        if self.starts_with("++") {
            self.pos += 2;
            self.skip_ws();
            let name = match self.peek_ident() {
                Some(n) => n.to_string(),
                None => return Err(ArithError::BadExpr),
            };
            self.pos += name.len();
            // check subscript
            self.skip_ws();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'[' {
                return Err(ArithError::BadExpr);
            }
            let cur = env.get(&name).unwrap_or(0);
            let new_val = cur.checked_add(1).ok_or(ArithError::Overflow)?;
            env.set(&name, new_val);
            return Ok(new_val);
        }
        if self.starts_with("--") {
            self.pos += 2;
            self.skip_ws();
            let name = match self.peek_ident() {
                Some(n) => n.to_string(),
                None => return Err(ArithError::BadExpr),
            };
            self.pos += name.len();
            self.skip_ws();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'[' {
                return Err(ArithError::BadExpr);
            }
            let cur = env.get(&name).unwrap_or(0);
            let new_val = cur.checked_sub(1).ok_or(ArithError::Overflow)?;
            env.set(&name, new_val);
            return Ok(new_val);
        }
        if self.bytes[self.pos] == b'+' {
            // avoid "++" already handled, "+=" handled at higher level but unary '+' is ok
            // if next char is '+', we already handled; if next is '=', should not consume as unary
            if self.starts_with("+=") {
                return Err(ArithError::BadExpr);
            }
            self.pos += 1;
            let v = self.parse_unary_inner(env)?;
            return Ok(v);
        }
        if self.bytes[self.pos] == b'-' {
            if self.starts_with("-=") {
                return Err(ArithError::BadExpr);
            }
            self.pos += 1;
            let v = self.parse_unary_inner(env)?;
            return match v.checked_neg() {
                Some(n) => Ok(n),
                None => Err(ArithError::Overflow),
            };
        }
        if self.bytes[self.pos] == b'!' {
            if self.starts_with("!=") {
                return Err(ArithError::BadExpr);
            }
            self.pos += 1;
            let v = self.parse_unary_inner(env)?;
            return Ok(if v == 0 { 1 } else { 0 });
        }
        if self.bytes[self.pos] == b'~' {
            self.pos += 1;
            let v = self.parse_unary_inner(env)?;
            return Ok(!v);
        }
        self.parse_postfix(env)
    }
    fn parse_postfix(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        // parse primary then check for ++/--
        let save = self.pos;
        // need to handle primary that is identifier with postfix
        // peek identifier
        self.skip_ws();
        if let Some(name) = self.peek_ident() {
            // look ahead: ident + optional ws + "++" or "--" and not part of assignment
            let after = self.pos + name.len();
            let ws_len = self.src[after..]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t' || *c == '\n' || *c == '\r')
                .map(|c| c.len_utf8())
                .sum::<usize>();
            let after_ws = after + ws_len;
            if self.src[after_ws..].starts_with("++") || self.src[after_ws..].starts_with("--") {
                // postfix form
                self.pos += name.len();
                self.skip_ws();
                let is_inc = self.starts_with("++");
                self.pos += 2;
                self.skip_ws();
                if self.pos < self.bytes.len() && self.bytes[self.pos] == b'[' {
                    return Err(ArithError::BadExpr);
                }
                let owned = name.to_string();
                let cur = env.get(&owned).unwrap_or(0);
                let new_val = if is_inc {
                    cur.checked_add(1).ok_or(ArithError::Overflow)?
                } else {
                    cur.checked_sub(1).ok_or(ArithError::Overflow)?
                };
                env.set(&owned, new_val);
                return Ok(cur);
            }
        }
        self.pos = save;
        self.parse_primary(env)
    }
    fn parse_primary(&mut self, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        self.skip_ws();
        if self.pos >= self.bytes.len() {
            return Err(ArithError::BadExpr);
        }
        let b = self.bytes[self.pos];
        if b == b'(' {
            self.pos += 1;
            let v = self.parse_comma(env)?;
            self.skip_ws();
            if self.pos >= self.bytes.len() || self.bytes[self.pos] != b')' {
                return Err(ArithError::BadExpr);
            }
            self.pos += 1;
            // check subscript after paren? not needed
            self.skip_ws();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'[' {
                return Err(ArithError::BadExpr);
            }
            return Ok(v);
        }
        if b == b'$' {
            return Err(ArithError::BadExpr);
        }
        if is_ident_start(b) {
            let name = match self.peek_ident() {
                Some(n) => n.to_string(),
                None => return Err(ArithError::BadExpr),
            };
            self.pos += name.len();
            self.skip_ws();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'[' {
                return Err(ArithError::BadExpr);
            }
            let v = env.get(&name).unwrap_or(0);
            return Ok(v);
        }
        if b.is_ascii_digit() {
            return self.parse_number();
        }
        Err(ArithError::BadExpr)
    }
    fn parse_number(&mut self) -> Result<i64, ArithError> {
        let start = self.pos;
        // Check prefixes
        if self.starts_with("0x") || self.starts_with("0X") {
            self.pos += 2;
            let hstart = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            if hstart == self.pos {
                return Err(ArithError::BadNumber);
            }
            let s = &self.src[hstart..self.pos];
            // reject trailing ident char
            if self.pos < self.bytes.len() && is_ident_char(self.bytes[self.pos]) {
                return Err(ArithError::BadNumber);
            }
            let mut val: i64 = 0;
            for ch in s.chars() {
                let digit = ch.to_digit(16).ok_or(ArithError::BadNumber)? as i64;
                val = val.checked_mul(16).ok_or(ArithError::Overflow)?;
                val = val.checked_add(digit).ok_or(ArithError::Overflow)?;
            }
            let _ = start;
            return Ok(val);
        }
        if self.starts_with("0b") || self.starts_with("0B") {
            self.pos += 2;
            let bstart = self.pos;
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos] == b'0' || self.bytes[self.pos] == b'1')
            {
                self.pos += 1;
            }
            if bstart == self.pos {
                return Err(ArithError::BadNumber);
            }
            if self.pos < self.bytes.len() && is_ident_char(self.bytes[self.pos]) {
                return Err(ArithError::BadNumber);
            }
            let mut val: i64 = 0;
            for ch in self.src[bstart..self.pos].chars() {
                let digit = if ch == '0' { 0 } else { 1 };
                val = val.checked_mul(2).ok_or(ArithError::Overflow)?;
                val = val.checked_add(digit).ok_or(ArithError::Overflow)?;
            }
            return Ok(val);
        }
        if self.starts_with("0o") || self.starts_with("0O") {
            self.pos += 2;
            let ostart = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                if self.bytes[self.pos] > b'7' {
                    return Err(ArithError::BadNumber);
                }
                self.pos += 1;
            }
            if ostart == self.pos {
                return Err(ArithError::BadNumber);
            }
            if self.pos < self.bytes.len() && is_ident_char(self.bytes[self.pos]) {
                return Err(ArithError::BadNumber);
            }
            let mut val: i64 = 0;
            for ch in self.src[ostart..self.pos].chars() {
                let digit = (ch as u8 - b'0') as i64;
                val = val.checked_mul(8).ok_or(ArithError::Overflow)?;
                val = val.checked_add(digit).ok_or(ArithError::Overflow)?;
            }
            return Ok(val);
        }
        // leading 0 octal vs decimal
        if self.bytes[self.pos] == b'0' {
            // count leading zeros/digits
            let dstart = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
            let s = &self.src[dstart..self.pos];
            if self.pos < self.bytes.len() && is_ident_char(self.bytes[self.pos]) {
                return Err(ArithError::BadNumber);
            }
            if s.len() > 1 {
                // octal interpretation: all digits must be 0-7
                let mut is_octal_ok = true;
                for ch in s.chars() {
                    if ch > '7' {
                        is_octal_ok = false;
                        break;
                    }
                }
                if !is_octal_ok {
                    return Err(ArithError::BadNumber);
                }
                let mut val: i64 = 0;
                for ch in s.chars() {
                    let digit = (ch as u8 - b'0') as i64;
                    val = val.checked_mul(8).ok_or(ArithError::Overflow)?;
                    val = val.checked_add(digit).ok_or(ArithError::Overflow)?;
                }
                return Ok(val);
            } else {
                return Ok(0);
            }
        }
        // decimal
        let dstart = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if dstart == self.pos {
            return Err(ArithError::BadExpr);
        }
        if self.pos < self.bytes.len() && is_ident_char(self.bytes[self.pos]) {
            return Err(ArithError::BadNumber);
        }
        let s = &self.src[dstart..self.pos];
        let mut val: i64 = 0;
        for ch in s.chars() {
            let digit = (ch as u8 - b'0') as i64;
            val = val.checked_mul(10).ok_or(ArithError::Overflow)?;
            val = val.checked_add(digit).ok_or(ArithError::Overflow)?;
        }
        Ok(val)
    }
    fn peek_ident(&mut self) -> Option<&'a str> {
        self.skip_ws();
        if self.pos >= self.bytes.len() {
            return None;
        }
        if !is_ident_start(self.bytes[self.pos]) {
            return None;
        }
        let start = self.pos;
        self.pos += 1;
        while self.pos < self.bytes.len() && is_ident_char(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let ident = &self.src[start..self.pos];
        self.pos = start; // reset, caller will consume
        Some(ident)
    }
}

fn is_ident_start(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'_')
}
fn is_ident_char(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
}

pub fn eval(src: &str, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
    // depth check for empty? still need to handle
    let mut p = Parser::new(src);
    p.skip_ws();
    if p.pos >= p.bytes.len() {
        return Err(ArithError::BadExpr);
    }
    let v = p.parse_comma(env)?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        return Err(ArithError::BadExpr);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MapEnv(HashMap<String, i64>);
    impl ArithEnv for MapEnv {
        fn get(&mut self, name: &str) -> Option<i64> {
            self.0.get(name).copied()
        }
        fn set(&mut self, name: &str, value: i64) {
            self.0.insert(name.to_string(), value);
        }
    }
    fn eval_str(s: &str) -> Result<i64, ArithError> {
        let mut env = MapEnv(HashMap::new());
        eval(s, &mut env)
    }
    fn eval_with_env(s: &str, env: &mut dyn ArithEnv) -> Result<i64, ArithError> {
        eval(s, env)
    }

    #[test]
    fn precedence_mul_add() {
        assert_eq!(eval_str("2+3*4").unwrap(), 14);
        assert_eq!(eval_str("2*3+4").unwrap(), 10);
    }
    #[test]
    fn precedence_shift() {
        assert_eq!(eval_str("1<<3+1").unwrap(), 16);
    }
    #[test]
    fn precedence_rel_eq() {
        assert_eq!(eval_str("1<2==1").unwrap(), 1);
    }
    #[test]
    fn bitwise() {
        assert_eq!(eval_str("6 & 3").unwrap(), 2);
        assert_eq!(eval_str("6 | 3").unwrap(), 7);
        assert_eq!(eval_str("6 ^ 3").unwrap(), 5);
    }
    #[test]
    fn logical() {
        assert_eq!(eval_str("0 || 2").unwrap(), 1);
        assert_eq!(eval_str("5 && 0").unwrap(), 0);
    }
    #[test]
    fn ternary() {
        assert_eq!(eval_str("1?2:3").unwrap(), 2);
        assert_eq!(eval_str("0?2:3").unwrap(), 3);
        assert_eq!(eval_str("1?0?1:2:3").unwrap(), 2);
    }
    #[test]
    fn comma() {
        assert_eq!(eval_str("1,2,3").unwrap(), 3);
    }
    #[test]
    fn unary() {
        assert_eq!(eval_str("-5").unwrap(), -5);
        assert_eq!(eval_str("!0").unwrap(), 1);
        assert_eq!(eval_str("~0").unwrap(), -1);
    }
    #[test]
    fn numbers() {
        assert_eq!(eval_str("42").unwrap(), 42);
        assert_eq!(eval_str("0x10").unwrap(), 16);
        assert_eq!(eval_str("0b101").unwrap(), 5);
        assert_eq!(eval_str("010").unwrap(), 8);
        assert_eq!(eval_str("0o10").unwrap(), 8);
    }
    #[test]
    fn div_zero() {
        assert_eq!(eval_str("1/0").unwrap_err(), ArithError::DivZero);
        assert_eq!(eval_str("1%0").unwrap_err(), ArithError::DivZero);
    }
    #[test]
    fn overflow() {
        assert_eq!(
            eval_str("9223372036854775807+1").unwrap_err(),
            ArithError::Overflow
        );
        assert_eq!(
            eval_str("9223372036854775807*2").unwrap_err(),
            ArithError::Overflow
        );
    }
    #[test]
    fn assignment() {
        let mut env = MapEnv(HashMap::new());
        assert_eq!(eval_with_env("a=5", &mut env).unwrap(), 5);
        assert_eq!(eval_with_env("a+2", &mut env).unwrap(), 7);
        assert_eq!(eval_with_env("a+=3", &mut env).unwrap(), 8);
        assert_eq!(eval_with_env("a", &mut env).unwrap(), 8);
    }
    #[test]
    fn inc_dec() {
        let mut env = MapEnv(HashMap::new());
        eval_with_env("x=5", &mut env).unwrap();
        assert_eq!(eval_with_env("++x", &mut env).unwrap(), 6);
        assert_eq!(eval_with_env("x++", &mut env).unwrap(), 6);
        assert_eq!(eval_with_env("x", &mut env).unwrap(), 7);
        assert_eq!(eval_with_env("--x", &mut env).unwrap(), 6);
        assert_eq!(eval_with_env("x--", &mut env).unwrap(), 6);
    }
    #[test]
    fn bad_expr() {
        assert_eq!(eval_str("").unwrap_err(), ArithError::BadExpr);
        assert_eq!(eval_str("(1").unwrap_err(), ArithError::BadExpr);
    }
    #[test]
    fn bad_number() {
        assert_eq!(eval_str("0x").unwrap_err(), ArithError::BadNumber);
        assert_eq!(eval_str("08").unwrap_err(), ArithError::BadNumber);
    }
    #[test]
    fn subscript_bad() {
        assert_eq!(eval_str("a[0]").unwrap_err(), ArithError::BadExpr);
    }
    #[test]
    fn deep_nesting() {
        let deep = "(".repeat(200) + "1" + &")".repeat(200);
        let err = eval_str(&deep).unwrap_err();
        assert_eq!(err, ArithError::Depth);
    }
    #[test]
    fn shift_overflow() {
        assert_eq!(eval_str("1<<100").unwrap_err(), ArithError::Overflow);
    }
    #[test]
    fn dollar_bad() {
        assert_eq!(eval_str("$a").unwrap_err(), ArithError::BadExpr);
    }
    #[test]
    fn nested_ternary() {
        assert_eq!(eval_str("1?2?3:4:5").unwrap(), 3);
    }
    #[test]
    fn compound_shift_assign() {
        let mut env = MapEnv(HashMap::new());
        eval_with_env("a=8", &mut env).unwrap();
        assert_eq!(eval_with_env("a<<=1", &mut env).unwrap(), 16);
        assert_eq!(eval_with_env("a>>=2", &mut env).unwrap(), 4);
    }
    #[test]
    fn pathological_no_panic() {
        let s = "1".repeat(10000);
        let r = eval_str(&s);
        assert!(r.is_err());
    }
}
