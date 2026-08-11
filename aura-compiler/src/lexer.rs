//! Lexer for Aura source code.

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
    /// Integer literal.
    IntLit(i32),
    /// Float literal.
    FloatLit(f64),
    /// String literal.
    StringLit(String),
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

    // Keywords
    /// `class`
    Class,
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
    /// `interface`
    Interface,
    /// `abstract`
    Abstract,
    /// `sealed`
    Sealed,
    /// `final`
    Final,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Token::Eof => "EOF",
            Token::Newline => "newline",
            Token::Ident(n) => return write!(f, "identifier `{}`", n),
            Token::IntLit(i) => return write!(f, "integer {}", i),
            Token::FloatLit(x) => return write!(f, "float {}", x),
            Token::StringLit(s) => return write!(f, "string {:?}", s),
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
            Token::Class => "`class`",
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
            Token::Interface => "`interface`",
            Token::Abstract => "`abstract`",
            Token::Sealed => "`sealed`",
            Token::Final => "`final`",
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
    pub fn lex(mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            if tok == Token::Eof {
                tokens.push(tok);
                break;
            }
            tokens.push(tok);
        }
        Ok(tokens)
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
            Some('"') => self.string(),
            Some(c) if c.is_ascii_digit() => self.number(),
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
                Ok(Token::Dot)
            }
            Some(':') => {
                self.advance();
                Ok(Token::Colon)
            }
            Some('?') => {
                self.advance();
                Ok(Token::Question)
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
            Some('&') => {
                self.advance();
                if self.match_char('&') {
                    Ok(Token::And)
                } else {
                    Err(self.error("unexpected character `&`"))
                }
            }
            Some('|') => {
                self.advance();
                if self.match_char('|') {
                    Ok(Token::Or)
                } else {
                    Err(self.error("unexpected character `|`"))
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
                        Some('.') => { self.advance(); Token::Dot }
                        Some(':') => { self.advance(); Token::Colon }
                        Some('?') => { self.advance(); Token::Question }
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
                .parse::<i32>()
                .map_err(|e| self.error(&e.to_string()))?;
            return Ok(Token::IntLit(value));
        }
        
        // Check if this is a float literal: . followed by a digit
        // But we need to handle tuple index chains like tuple.0.0
        // In that case, we want IntLit(0), Dot, IntLit(0), not IntLit(0), FloatLit(0.0)
        // So we check if the fractional part is followed by another .
        // If so, this is likely a tuple index chain, not a float
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
                        // This is a tuple index chain, not a float
                        return Ok(self.current.parse::<i32>().map(Token::IntLit).map_err(|e| self.error(&e.to_string()))?);
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
            Ok(Token::FloatLit(value))
        } else {
            let value = self
                .current
                .parse::<i32>()
                .map_err(|e| self.error(&e.to_string()))?;
            Ok(Token::IntLit(value))
        }
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
            "interface" => Token::Interface,
            "abstract" => Token::Abstract,
            "sealed" => Token::Sealed,
            "final" => Token::Final,
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
