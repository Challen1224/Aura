//! Lexer for Aura source code.

use crate::ast::{FloatSuffix, IntSuffix};
use std::fmt;

/// Token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// End of input.
    Eof,
    /// Newline (significant for statement separation).
    Newline,
    /// Identifier or keyword.
    Ident(String),
    /// Integer literal with optional type suffix.
    IntLit(i64, IntSuffix),
    /// Float literal with optional type suffix.
    FloatLit(f64, FloatSuffix),
    /// String literal.
    StringLit(String),
    /// Character literal (Unicode scalar value).
    CharLit(char),
    /// Start of string interpolation `{`
    InterpStart,
    /// End of string interpolation `}`
    InterpEnd,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `;`
    Semi,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `:`
    Colon,
    /// `=`
    Assign,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `!`
    Bang,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `|`
    Pipe,
    /// A custom operator symbol (starts with `|`, `&`, or `^`; two or
    /// more characters; `&&`/`||` stay reserved). Resolved to an
    /// `operator<sym>` overload by the typer.
    CustomOp(String),
    /// `..`
    DotDot,
    /// `..=`
    DotDotEq,

    // Keywords
    /// `class`
    Class,
    /// `type`
    Type,
    /// `newtype`
    Newtype,
    /// `is`
    Is,
    /// `var`
    Var,
    /// `static`
    Static,
    /// `void`
    Void,
    /// `int`
    Int,
    /// `float`
    Float,
    /// `bool`
    Bool,
    /// `string`
    String,
    /// `true`
    True,
    /// `false`
    False,
    /// `null`
    Null,
    /// `if`
    If,
    /// `else`
    Else,
    /// `while`
    While,
    /// `return`
    Return,
    /// `new`
    New,
    /// `for`
    For,
    /// `break`
    Break,
    /// `continue`
    Continue,
    /// `do`
    Do,
    /// `?`
    Question,
    /// `match`
    Match,
    /// `enum`
    Enum,
    /// `=>`
    FatArrow,
    /// `super`
    Super,
    /// `virtual`
    Virtual,
    /// `override`
    Override,
    /// `protected`
    Protected,
    /// `private`
    Private,
    /// `internal`
    Internal,
    /// `interface`
    Interface,
    /// `record`
    Record,
    /// `abstract`
    Abstract,
    /// `sealed`
    Sealed,
    /// `final`
    Final,
    /// `throw`
    Throw,
    /// `try`
    Try,
    /// `catch`
    Catch,
    /// `finally`
    Finally,
    /// `using`
    Using,
    /// `in`
    In,
    /// `let`
    Let,
    /// `with`
    With,
    /// `as` (explicit cast).
    As,
    /// `??`
    NullCoalesce,
    /// `?.`
    NullConditional,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Token::Eof => "EOF",
            Token::Newline => "newline",
            Token::Ident(n) => return write!(f, "identifier `{}`", n),
            Token::IntLit(i, _) => return write!(f, "integer {}", i),
            Token::FloatLit(x, _) => return write!(f, "float {}", x),
            Token::StringLit(s) => return write!(f, "string {:?}", s),
            Token::CharLit(c) => return write!(f, "character {:?}", c),
            Token::InterpStart => "`{` (interpolation)",
            Token::InterpEnd => "`}` (interpolation)",
            Token::Plus => "`+`",
            Token::Minus => "`-`",
            Token::Star => "`*`",
            Token::Slash => "`/`",
            Token::Percent => "`%`",
            Token::LParen => "`(`",
            Token::RParen => "`)`",
            Token::LBrace => "`{`",
            Token::RBrace => "`}`",
            Token::Semi => "`;`",
            Token::Comma => "`,`",
            Token::Dot => "`.`",
            Token::Colon => "`:`",
            Token::Assign => "`=`",
            Token::Eq => "`==`",
            Token::Ne => "`!=`",
            Token::Lt => "`<`",
            Token::Le => "`<=`",
            Token::Gt => "`>`",
            Token::Ge => "`>=`",
            Token::Bang => "`!`",
            Token::And => "`&&`",
            Token::Or => "`||`",
            Token::Pipe => "`|`",
            Token::CustomOp(s) => return write!(f, "`{}`", s),
            Token::DotDot => "`..`",
            Token::DotDotEq => "`..=`",
            Token::Class => "`class`",
            Token::Type => "`type`",
            Token::Newtype => "`newtype`",
            Token::Is => "`is`",
            Token::Var => "`var`",
            Token::Static => "`static`",
            Token::Void => "`void`",
            Token::Int => "`int`",
            Token::Float => "`float`",
            Token::Bool => "`bool`",
            Token::String => "`string`",
            Token::True => "`true`",
            Token::False => "`false`",
            Token::Null => "`null`",
            Token::If => "`if`",
            Token::Else => "`else`",
            Token::While => "`while`",
            Token::Return => "`return`",
            Token::New => "`new`",
            Token::For => "`for`",
            Token::Break => "`break`",
            Token::Continue => "`continue`",
            Token::Do => "`do`",
            Token::Question => "`?`",
            Token::Match => "`match`",
            Token::Enum => "`enum`",
            Token::FatArrow => "`=>`",
            Token::Super => "`super`",
            Token::Virtual => "`virtual`",
            Token::Override => "`override`",
            Token::Protected => "`protected`",
            Token::Private => "`private`",
            Token::Internal => "`internal`",
            Token::Interface => "`interface`",
            Token::Record => "`record`",
            Token::Abstract => "`abstract`",
            Token::Sealed => "`sealed`",
            Token::Final => "`final`",
            Token::Throw => "`throw`",
            Token::Try => "`try`",
            Token::Catch => "`catch`",
            Token::Finally => "`finally`",
            Token::Using => "`using`",
            Token::In => "`in`",
            Token::Let => "`let`",
            Token::With => "`with`",
            Token::As => "`as`",
            Token::NullCoalesce => "`??`",
            Token::NullConditional => "`?.`",
        };
        write!(f, "{}", s)
    }
}

/// Lexer state.
pub struct Lexer<'a> {
    source: &'a str,
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: usize,
    col: usize,
    current: String,
    after_dot: bool,
    pending_tokens: Vec<Token>,
}

impl<'a> Lexer<'a> {
    /// Create a lexer for the source string.
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            chars: source.chars().peekable(),
            line: 1,
            col: 0,
            current: String::new(),
            after_dot: false,
            pending_tokens: Vec::new(),
        }
    }

    /// Tokenize the entire source.
    /// Tokenize the source. Returns the tokens plus, for each token, the
    /// 1-based line it starts on (used for error locations).
    pub fn lex(mut self) -> Result<(Vec<Token>, Vec<usize>), String> {
        let mut tokens = Vec::new();
        let mut lines = Vec::new();
        loop {
            let tok = self.next_token()?;
            let done = tok == Token::Eof;
            tokens.push(tok);
            lines.push(self.line);
            if done {
                break;
            }
        }
        Ok((tokens, lines))
    }

    fn next_token(&mut self) -> Result<Token, String> {
        // Return pending tokens first
        if !self.pending_tokens.is_empty() {
            return Ok(self.pending_tokens.remove(0));
        }

        self.skip_whitespace();
        self.current.clear();

        let tok = match self.peek() {
            None => Ok(Token::Eof),
            Some('\n') => {
                self.advance();
                self.line += 1;
                self.col = 0;
                Ok(Token::Newline)
            }
            Some('/') if self.peek_next() == Some('/') => {
                self.skip_line_comment();
                self.next_token()
            }
            Some('/') if self.peek_next() == Some('*') => {
                self.skip_block_comment()?;
                self.next_token()
            }
            Some('"') if self.peek_next() == Some('"') && self.peek_next_next() == Some('"') => {
                self.multiline_string()
            }
            Some('"') => self.string(),
            Some('\'') => self.char_lit(),
            Some(c) if c.is_ascii_digit() => self.number(),
            Some('r') if self.peek_next() == Some('"') => self.raw_string(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => self.identifier(),
            Some('+') => {
                self.advance();
                Ok(Token::Plus)
            }
            Some('-') => {
                self.advance();
                Ok(Token::Minus)
            }
            Some('*') => {
                self.advance();
                Ok(Token::Star)
            }
            Some('/') => {
                self.advance();
                Ok(Token::Slash)
            }
            Some('%') => {
                self.advance();
                Ok(Token::Percent)
            }
            Some('(') => {
                self.advance();
                Ok(Token::LParen)
            }
            Some(')') => {
                self.advance();
                Ok(Token::RParen)
            }
            Some('{') => {
                self.advance();
                Ok(Token::LBrace)
            }
            Some('}') => {
                self.advance();
                Ok(Token::RBrace)
            }
            Some(';') => {
                self.advance();
                Ok(Token::Semi)
            }
            Some(',') => {
                self.advance();
                Ok(Token::Comma)
            }
            Some('.') => {
                self.advance();
                if self.match_char('.') {
                    if self.match_char('=') {
                        Ok(Token::DotDotEq)
                    } else {
                        Ok(Token::DotDot)
                    }
                } else {
                    Ok(Token::Dot)
                }
            }
            Some(':') => {
                self.advance();
                Ok(Token::Colon)
            }
            Some('?') => {
                self.advance();
                if self.match_char('?') {
                    Ok(Token::NullCoalesce)
                } else if self.match_char('.') {
                    Ok(Token::NullConditional)
                } else {
                    Ok(Token::Question)
                }
            }
            Some('=') => {
                self.advance();
                if self.match_char('=') {
                    Ok(Token::Eq)
                } else if self.match_char('>') {
                    Ok(Token::FatArrow)
                } else {
                    Ok(Token::Assign)
                }
            }
            Some('!') => {
                self.advance();
                if self.match_char('=') {
                    Ok(Token::Ne)
                } else {
                    Ok(Token::Bang)
                }
            }
            Some('<') => {
                self.advance();
                if self.match_char('=') {
                    Ok(Token::Le)
                } else {
                    Ok(Token::Lt)
                }
            }
            Some('>') => {
                self.advance();
                if self.match_char('=') {
                    Ok(Token::Ge)
                } else {
                    Ok(Token::Gt)
                }
            }
            // `|`, `&`, `^` begin custom operators: munch the maximal run
            // of operator characters. `&&`/`||` stay themselves; a run of
            // one keeps the historical single-char behavior. Runs cannot
            // start with `<`/`>` (generics own those), so nested-generic
            // `>>` and comparisons are untouched.
            Some(c @ ('&' | '|' | '^')) => {
                self.advance();
                let mut sym = String::from(c);
                while let Some(n) = self.peek() {
                    if matches!(n, '|' | '&' | '^' | '+' | '-' | '*' | '/' | '%' | '<' | '>' | '=' | '!' | '~' | '?') {
                        sym.push(n);
                        self.advance();
                    } else {
                        break;
                    }
                }
                match sym.as_str() {
                    "&&" => Ok(Token::And),
                    "||" => Ok(Token::Or),
                    "|" => Ok(Token::Pipe),
                    "&" => Err(self.error("unexpected character `&`")),
                    "^" => Err(self.error("unexpected character `^`")),
                    _ => Ok(Token::CustomOp(sym)),
                }
            }
            Some(c) => Err(self.error(&format!("unexpected character `{}`", c))),
        };
        
        // Track whether we just produced a Dot token
        if let Ok(ref t) = tok {
            self.after_dot = matches!(t, Token::Dot);
        }
        
        tok
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' || c == '\r' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.advance();
        }
    }

    fn skip_block_comment(&mut self) -> Result<(), String> {
        self.advance(); // '/'
        self.advance(); // '*'
        let mut depth = 1;
        while depth > 0 {
            match (self.peek(), self.peek_next()) {
                (Some('/'), Some('*')) => {
                    self.advance();
                    self.advance();
                    depth += 1;
                }
                (Some('*'), Some('/')) => {
                    self.advance();
                    self.advance();
                    depth -= 1;
                }
                (Some('\n'), _) => {
                    self.advance();
                    self.line += 1;
                    self.col = 0;
                }
                (Some(_), _) => {
                    self.advance();
                }
                (None, _) => return Err(self.error("unterminated block comment")),
            }
        }
        Ok(())
    }

    fn string(&mut self) -> Result<Token, String> {
        self.advance(); // opening quote
        let mut value = String::new();
        let mut tokens = Vec::new();
        
        while let Some(c) = self.peek() {
            if c == '"' {
                self.advance();
                // Add the final string part
                tokens.push(Token::StringLit(value));
                
                // If we only have one token, return it directly
                if tokens.len() == 1 {
                    return Ok(tokens.remove(0));
                }
                
                // Otherwise, add all but the first to pending and return the first
                let first = tokens.remove(0);
                self.pending_tokens.extend(tokens);
                return Ok(first);
            }
            if c == '\\' {
                self.advance();
                match self.peek() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('\\') => value.push('\\'),
                    Some('"') => value.push('"'),
                    Some('{') => value.push('{'),
                    Some('}') => value.push('}'),
                    Some(other) => return Err(self.error(&format!("invalid escape `\\{}`", other))),
                    None => return Err(self.error("unterminated string")),
                }
                self.advance();
            } else if c == '{' {
                // Start of interpolation
                // Add the string part before the interpolation
                tokens.push(Token::StringLit(value.clone()));
                value.clear();
                tokens.push(Token::InterpStart);
                self.advance(); // consume '{'
                
                // Parse tokens until we find the matching '}'
                let mut brace_depth = 1;
                while brace_depth > 0 {
                    self.skip_whitespace();
                    self.current.clear();
                    
                    let tok = match self.peek() {
                        None => return Err(self.error("unterminated string interpolation")),
                        Some('{') => {
                            self.advance();
                            brace_depth += 1;
                            Token::LBrace
                        }
                        Some('}') => {
                            self.advance();
                            brace_depth -= 1;
                            if brace_depth == 0 {
                                tokens.push(Token::InterpEnd);
                                break;
                            }
                            Token::RBrace
                        }
                        Some(c) if c.is_ascii_digit() => self.number()?,
                        Some(c) if c.is_ascii_alphabetic() || c == '_' => self.identifier()?,
                        Some('"') => {
                            // Recursively parse nested string
                            let nested_tok = self.string()?;
                            // If the nested string returned multiple tokens, we need to handle them
                            tokens.push(nested_tok);
                            // Add any pending tokens from the nested string
                            while !self.pending_tokens.is_empty() {
                                tokens.push(self.pending_tokens.remove(0));
                            }
                            continue;
                        }
                        Some('+') => { self.advance(); Token::Plus }
                        Some('-') => { self.advance(); Token::Minus }
                        Some('*') => { self.advance(); Token::Star }
                        Some('/') => { self.advance(); Token::Slash }
                        Some('%') => { self.advance(); Token::Percent }
                        Some('(') => { self.advance(); Token::LParen }
                        Some(')') => { self.advance(); Token::RParen }
                        Some(';') => { self.advance(); Token::Semi }
                        Some(',') => { self.advance(); Token::Comma }
                        Some('.') => {
                            self.advance();
                            if self.match_char('.') {
                                if self.match_char('=') {
                                    Token::DotDotEq
                                } else {
                                    Token::DotDot
                                }
                            } else {
                                Token::Dot
                            }
                        }
                        Some(':') => { self.advance(); Token::Colon }
                        Some('?') => {
                            self.advance();
                            if self.match_char('?') {
                                Token::NullCoalesce
                            } else if self.match_char('.') {
                                Token::NullConditional
                            } else {
                                Token::Question
                            }
                        }
                        Some('=') => {
                            self.advance();
                            if self.match_char('=') {
                                Token::Eq
                            } else if self.match_char('>') {
                                Token::FatArrow
                            } else {
                                Token::Assign
                            }
                        }
                        Some('!') => {
                            self.advance();
                            if self.match_char('=') {
                                Token::Ne
                            } else {
                                Token::Bang
                            }
                        }
                        Some('<') => {
                            self.advance();
                            if self.match_char('=') {
                                Token::Le
                            } else {
                                Token::Lt
                            }
                        }
                        Some('>') => {
                            self.advance();
                            if self.match_char('=') {
                                Token::Ge
                            } else {
                                Token::Gt
                            }
                        }
                        Some('&') => {
                            self.advance();
                            if self.match_char('&') {
                                Token::And
                            } else {
                                return Err(self.error("unexpected character `&`"));
                            }
                        }
                        Some('|') => {
                            self.advance();
                            if self.match_char('|') {
                                Token::Or
                            } else {
                                return Err(self.error("unexpected character `|`"));
                            }
                        }
                        Some(c) => return Err(self.error(&format!("unexpected character `{}` in interpolation", c))),
                    };
                    tokens.push(tok);
                }
                
                // Continue parsing the rest of the string
            } else if c == '\n' {
                return Err(self.error("unterminated string"));
            } else {
                value.push(c);
                self.advance();
            }
        }
        Err(self.error("unterminated string"))
    }

    /// Parse a character literal: `'a'`, `'\n'`, `'\u{1F600}'`.
    fn char_lit(&mut self) -> Result<Token, String> {
        self.advance(); // opening quote
        let c = match self.peek() {
            Some('\\') => {
                self.advance(); // backslash
                self.read_escape()?
            }
            Some(c) => {
                self.advance();
                c
            }
            None => return Err(self.error("unterminated character literal")),
        };
        if self.peek() != Some('\'') {
            return Err(self.error("character literal must contain exactly one character"));
        }
        self.advance(); // closing quote
        Ok(Token::CharLit(c))
    }

    /// Parse a raw string literal: `r"..."`. Content is taken verbatim (no
    /// escapes, no interpolation) and may span multiple lines. Ends at the
    /// first `"` after the opening quote.
    fn raw_string(&mut self) -> Result<Token, String> {
        self.advance(); // 'r'
        self.advance(); // opening quote
        let mut value = String::new();
        while let Some(c) = self.peek() {
            if c == '"' {
                self.advance();
                return Ok(Token::StringLit(value));
            }
            if c == '\n' {
                self.line += 1;
                self.col = 0;
            }
            value.push(c);
            self.advance();
        }
        Err(self.error("unterminated raw string"))
    }

    /// Parse a multi-line string literal: `"""..."""`. Escape sequences are
    /// processed but there is no interpolation; the string ends at the first
    /// run of three closing quotes.
    fn multiline_string(&mut self) -> Result<Token, String> {
        self.advance(); // opening '"'
        self.advance(); // opening '"'
        self.advance(); // opening '"'
        let mut value = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated multi-line string")),
                Some('"') if self.peek_next() == Some('"') && self.peek_next_next() == Some('"') => {
                    self.advance();
                    self.advance();
                    self.advance();
                    return Ok(Token::StringLit(value));
                }
                Some('"') => {
                    value.push('"');
                    self.advance();
                }
                Some('\\') => {
                    self.advance(); // backslash
                    if self.peek().is_none() {
                        return Err(self.error("unterminated multi-line string"));
                    }
                    value.push(self.read_escape()?);
                }
                Some(c) => {
                    if c == '\n' {
                        self.line += 1;
                        self.col = 0;
                    }
                    value.push(c);
                    self.advance();
                }
            }
        }
    }

    /// Parse an escape sequence; the backslash has already been consumed and
    /// `peek()` points at the first character after it.
    fn read_escape(&mut self) -> Result<char, String> {
        let c = match self.peek() {
            Some('n') => '\n',
            Some('t') => '\t',
            Some('r') => '\r',
            Some('0') => '\0',
            Some('\\') => '\\',
            Some('\'') => '\'',
            Some('"') => '"',
            Some('{') => '{',
            Some('}') => '}',
            Some('u') => {
                self.advance(); // consume 'u'
                if self.peek() != Some('{') {
                    return Err(self.error("expected `{` after `\\u`"));
                }
                self.advance();
                let mut hex = String::new();
                while let Some(h) = self.peek() {
                    if h == '}' {
                        break;
                    }
                    if !h.is_ascii_hexdigit() {
                        return Err(self.error("invalid hex digit in `\\u{...}` escape"));
                    }
                    hex.push(h);
                    self.advance();
                }
                if self.peek() != Some('}') {
                    return Err(self.error("unterminated `\\u{...}` escape"));
                }
                self.advance(); // consume '}'
                let scalar = u32::from_str_radix(&hex, 16)
                    .map_err(|_| self.error("invalid `\\u{...}` escape"))?;
                return char::from_u32(scalar)
                    .ok_or_else(|| self.error("`\\u{...}` escape is not a valid Unicode scalar value"));
            }
            Some(other) => return Err(self.error(&format!("invalid escape `\\{}`", other))),
            None => return Err(self.error("unterminated escape")),
        };
        self.advance();
        Ok(c)
    }

    fn number(&mut self) -> Result<Token, String> {
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
        // If we're after a dot (e.g., tuple.0.0), don't parse floats.
        // This allows tuple index chains like tuple.0.0 to be lexed as
        // Ident("tuple"), Dot, IntLit(0), Dot, IntLit(0)
        // instead of Ident("tuple"), Dot, FloatLit(0.0)
        if self.after_dot {
            let value = self
                .current
                .parse::<i64>()
                .map_err(|e| self.error(&e.to_string()))?;
            return Ok(Token::IntLit(value, IntSuffix::None));
        }

        // Check if this is a float literal: . followed by a digit
        // But we need to handle tuple index chains like tuple.0.0
        // In that case, we want IntLit(0), Dot, IntLit(0), not IntLit(0), FloatLit(0.0)
        // So we check if the fractional part is followed by another .
        // If so, this is likely a tuple index chain, not a float
        // UNLESS it's followed by .. or ..= (range operators)
        let is_float = if self.peek() == Some('.') && self.peek_next().map_or(false, |c| c.is_ascii_digit()) {
            // Look ahead to see if the fractional part is followed by another .
            let mut lookahead = self.chars.clone();
            lookahead.next(); // skip the .
            // Skip all digits in the fractional part
            while let Some(c) = lookahead.next() {
                if c.is_ascii_digit() {
                    continue;
                } else {
                    // Check if this character is a .
                    if c == '.' {
                        // Check if it's .. or ..= (range operator)
                        let mut lookahead2 = lookahead.clone();
                        if let Some(next) = lookahead2.next() {
                            if next == '.' || next == '=' {
                                // This is a range operator, so treat as float
                                break;
                            }
                        }
                        // This is a tuple index chain, not a float
                        return Ok(self.current.parse::<i64>().map(|v| Token::IntLit(v, IntSuffix::None)).map_err(|e| self.error(&e.to_string()))?);
                    } else {
                        // This is a normal float
                        break;
                    }
                }
            }
            true
        } else {
            false
        };

        if is_float {
            self.advance(); // consume the .
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
            let value = self
                .current
                .parse::<f64>()
                .map_err(|e| self.error(&e.to_string()))?;
            let suffix = self.read_numeric_suffix();
            let suffix = match suffix.as_deref() {
                None => FloatSuffix::None,
                Some("f32") | Some("F32") => FloatSuffix::F32,
                Some("f64") | Some("F64") => FloatSuffix::F64,
                Some(other) => return Err(self.error(&format!("invalid float literal suffix `{}`", other))),
            };
            Ok(Token::FloatLit(value, suffix))
        } else {
            let value = self
                .current
                .parse::<i64>()
                .map_err(|e| self.error(&e.to_string()))?;
            let suffix = self.read_numeric_suffix();
            let suffix = match suffix.as_deref() {
                None => IntSuffix::None,
                Some("i8") | Some("I8") => IntSuffix::I8,
                Some("i16") | Some("I16") => IntSuffix::I16,
                Some("i32") | Some("I32") => IntSuffix::I32,
                Some("i64") | Some("I64") => IntSuffix::I64,
                Some("u8") | Some("U8") => IntSuffix::U8,
                Some("u16") | Some("U16") => IntSuffix::U16,
                Some("u32") | Some("U32") => IntSuffix::U32,
                Some("u64") | Some("U64") => IntSuffix::U64,
                Some(other) => return Err(self.error(&format!("invalid integer literal suffix `{}`", other))),
            };
            Ok(Token::IntLit(value, suffix))
        }
    }

    /// If the next characters form an alphabetic literal suffix (e.g. `u8`),
    /// consume and return it. Otherwise return None.
    fn read_numeric_suffix(&mut self) -> Option<String> {
        if !self.peek().map_or(false, |c| c.is_ascii_alphabetic()) {
            return None;
        }
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() || c.is_ascii_digit() {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        Some(s)
    }

    fn identifier(&mut self) -> Result<Token, String> {
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }
        let word = self.current.as_str();
        Ok(match word {
            "class" => Token::Class,
            "type" => Token::Type,
            "newtype" => Token::Newtype,
            "is" => Token::Is,
            "var" => Token::Var,
            "static" => Token::Static,
            "void" => Token::Void,
            "int" => Token::Int,
            "float" => Token::Float,
            "bool" => Token::Bool,
            "string" => Token::String,
            "true" => Token::True,
            "false" => Token::False,
            "null" => Token::Null,
            "if" => Token::If,
            "else" => Token::Else,
            "while" => Token::While,
            "return" => Token::Return,
            "new" => Token::New,
            "for" => Token::For,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "do" => Token::Do,
            "match" => Token::Match,
            "enum" => Token::Enum,
            "super" => Token::Super,
            "virtual" => Token::Virtual,
            "override" => Token::Override,
            "protected" => Token::Protected,
            "private" => Token::Private,
            "internal" => Token::Internal,
            "interface" => Token::Interface,
            "record" => Token::Record,
            "abstract" => Token::Abstract,
            "sealed" => Token::Sealed,
            "final" => Token::Final,
            "throw" => Token::Throw,
            "try" => Token::Try,
            "catch" => Token::Catch,
            "finally" => Token::Finally,
            "using" => Token::Using,
            "in" => Token::In,
            "let" => Token::Let,
            "with" => Token::With,
            "as" => Token::As,
            _ => Token::Ident(word.to_string()),
        })
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.next();
        if let Some(ch) = c {
            self.current.push(ch);
            self.col += 1;
        }
        c
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn peek_next(&mut self) -> Option<char> {
        let mut it = self.chars.clone();
        it.next();
        it.peek().copied()
    }

    fn peek_next_next(&mut self) -> Option<char> {
        let mut it = self.chars.clone();
        it.next();
        it.next();
        it.peek().copied()
    }

    fn match_char(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error(&self, msg: &str) -> String {
        format!("{} at line {}, col {}", msg, self.line, self.col)
    }
}
