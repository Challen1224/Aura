//! Recursive-descent parser for Aura.

use crate::ast::*;
use crate::lexer::Token;

/// Parser state.
pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    /// Create a parser over a token slice.
    pub fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    /// Parse the token stream into a program AST.
    pub fn parse(mut self) -> Result<Program, String> {
        let mut decls = Vec::new();
        while !self.is_at_end() {
            self.skip_newlines();
            if self.is_at_end() {
                break;
            }
            if self.check(Token::Enum) {
                decls.push(Decl::Enum(self.parse_enum()?));
            } else if self.match_token(Token::Interface) {
                decls.push(Decl::Class(self.parse_class(true, false, false)?));
            } else if self.match_token(Token::Abstract) {
                let is_sealed = self.match_token(Token::Sealed);
                self.consume(Token::Class, "expected `class` after `abstract`")?;
                decls.push(Decl::Class(self.parse_class(false, true, is_sealed)?));
            } else if self.match_token(Token::Sealed) {
                let is_abstract = self.match_token(Token::Abstract);
                self.consume(Token::Class, "expected `class` after `sealed`")?;
                decls.push(Decl::Class(self.parse_class(false, is_abstract, true)?));
            } else {
                self.consume(Token::Class, "expected `class` or `interface`")?;
                decls.push(Decl::Class(self.parse_class(false, false, false)?));
            }
        }
        Ok(Program { decls })
    }

    /// Parse the body of a class/interface declaration, assuming the leading
    /// `class`/`interface` keyword has already been consumed.
    fn parse_class(
        &mut self,
        is_interface: bool,
        is_abstract: bool,
        is_sealed: bool,
    ) -> Result<ClassDecl, String> {
        let name = self.consume_ident("expected class name")?;
        let generic_params = if self.check(Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };
        let bases = if self.match_token(Token::Colon) {
            let mut bases = Vec::new();
            loop {
                let base = self.consume_ident("expected base type name after `:`")?;
                if base == name {
                    return Err(format!("`{}` cannot inherit from itself", name));
                }
                if bases.contains(&base) {
                    return Err(format!("`{}` lists base type `{}` more than once", name, base));
                }
                bases.push(base);
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
            bases
        } else {
            Vec::new()
        };
        self.consume(Token::LBrace, "expected `{`")?;
        let mut members = Vec::new();
        while !self.check(Token::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(Token::RBrace) {
                break;
            }
            members.push(self.parse_member(is_interface)?);
        }
        self.consume(Token::RBrace, "expected `}`")?;
        Ok(ClassDecl {
            name,
            generic_params,
            bases,
            is_interface,
            is_abstract,
            is_sealed,
            members,
        })
    }

    fn parse_enum(&mut self) -> Result<EnumDecl, String> {
        self.consume(Token::Enum, "expected `enum`")?;
        let name = self.consume_ident("expected enum name")?;
        self.consume(Token::LBrace, "expected `{`")?;
        let mut variants = Vec::new();
        while !self.check(Token::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(Token::RBrace) {
                break;
            }
            let variant_name = self.consume_ident("expected variant name")?;
            let fields = if self.check(Token::LParen) {
                self.consume(Token::LParen, "expected `(`")?;
                let mut fields = Vec::new();
                if !self.check(Token::RParen) {
                    loop {
                        let ty = self.parse_type()?;
                        let field_name = self.consume_ident("expected field name")?;
                        fields.push(EnumVariantField { ty, name: field_name });
                        if !self.match_token(Token::Comma) {
                            break;
                        }
                    }
                }
                self.consume(Token::RParen, "expected `)`")?;
                fields
            } else {
                Vec::new()
            };
            variants.push(EnumVariant { name: variant_name, fields });
            self.skip_newlines();
            if !self.check(Token::RBrace) {
                self.match_token(Token::Comma);
            }
            self.skip_newlines();
        }
        self.consume(Token::RBrace, "expected `}`")?;
        Ok(EnumDecl { name, variants })
    }

    fn parse_generic_params(&mut self) -> Result<Vec<GenericParam>, String> {
        self.consume(Token::Lt, "expected `<`")?;
        let mut params = Vec::new();
        loop {
            let name = self.consume_ident("expected generic parameter name")?;
            let constraint = if self.match_token(Token::Colon) {
                Some(self.parse_type()?)
            } else {
                None
            };
            params.push(GenericParam { name, constraint });
            if !self.match_token(Token::Comma) {
                break;
            }
        }
        self.consume(Token::Gt, "expected `>`")?;
        Ok(params)
    }

    fn parse_member(&mut self, in_interface: bool) -> Result<Member, String> {
        let mut is_static = false;
        let mut visibility = Visibility::Public;
        let mut is_virtual = false;
        let mut is_override = false;
        let mut is_abstract = false;
        let mut is_final = false;
        loop {
            if self.match_token(Token::Static) {
                is_static = true;
            } else if self.match_token(Token::Protected) {
                visibility = Visibility::Protected;
            } else if self.match_token(Token::Virtual) {
                is_virtual = true;
            } else if self.match_token(Token::Override) {
                is_override = true;
            } else if self.match_token(Token::Abstract) {
                is_abstract = true;
            } else if self.match_token(Token::Final) {
                is_final = true;
            } else if self.match_token(Token::Sealed) {
                return Err("`sealed` can only be applied to a class declaration".to_string());
            } else {
                break;
            }
        }
        let generic_params = if self.check(Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };
        let ty = self.parse_type()?;
        let name = self.consume_ident("expected member name")?;

        if self.check(Token::LParen) {
            let params = self.parse_params()?;
            let body = if is_abstract || (in_interface && !self.check(Token::LBrace)) {
                if is_static || is_virtual {
                    return Err(format!("abstract method `{}` cannot be {}",
                        name, if is_static { "static" } else { "virtual" }));
                }
                self.consume(Token::Semi, "expected `;` after abstract method")?;
                Vec::new()
            } else {
                self.consume(Token::LBrace, "expected `{`")?;
                self.parse_block()?
            };
            Ok(Member::Method(MethodDecl {
                is_static,
                visibility,
                is_virtual,
                is_override,
                is_abstract,
                is_final,
                generic_params,
                return_ty: ty,
                name,
                params,
                body,
            }))
        } else {
            if is_virtual || is_override || is_abstract || is_final {
                return Err(format!("`{}` cannot be used on a field",
                    if is_abstract { "abstract" } else if is_virtual { "virtual" } else if is_override { "override" } else { "final" }));
            }
            self.consume(Token::Semi, "expected `;`")?;
            Ok(Member::Field(FieldDecl {
                is_static,
                visibility,
                ty,
                name,
            }))
        }
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, String> {
        self.consume(Token::LParen, "expected `(`")?;
        let mut params = Vec::new();
        if !self.check(Token::RParen) {
            loop {
                let ty = self.parse_type()?;
                let name = self.consume_ident("expected parameter name")?;
                params.push(Param { ty, name });
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
        }
        self.consume(Token::RParen, "expected `)`")?;
        Ok(params)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        while !self.check(Token::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(Token::RBrace) {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        self.consume(Token::RBrace, "expected `}`")?;
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        self.skip_newlines();
        if self.match_token(Token::If) {
            self.parse_if()
        } else if self.match_token(Token::While) {
            self.parse_while()
        } else if self.match_token(Token::For) {
            self.parse_for()
        } else if self.match_token(Token::Do) {
            self.parse_do_while()
        } else if self.match_token(Token::Break) {
            self.consume_semi()?;
            Ok(Stmt::Break)
        } else if self.match_token(Token::Continue) {
            self.consume_semi()?;
            Ok(Stmt::Continue)
        } else if self.match_token(Token::Return) {
            self.parse_return()
        } else if self.check(Token::LParen) && self.peek_ahead_is_tuple_decl() {
            self.parse_tuple_decl()
        } else if self.check_type() && self.peek_ahead_is_ident_after_type() {
            self.parse_var_decl()
        } else if self.check(Token::LBrace) {
            self.advance();
            Ok(Stmt::Block(self.parse_block()?))
        } else {
            let expr = self.parse_expr()?;
            if self.match_token(Token::Assign) {
                let target = expr_to_assign_target(expr)?;
                let value = self.parse_expr()?;
                self.consume_semi()?;
                Ok(Stmt::Assign(target, value))
            } else {
                self.consume_semi()?;
                Ok(Stmt::Expr(expr))
            }
        }
    }

    fn peek_ahead_is_tuple_decl(&self) -> bool {
        let mut pos = self.pos;
        if pos >= self.tokens.len() || !matches!(self.tokens.get(pos), Some(Token::LParen)) {
            return false;
        }
        pos += 1;
        
        // Find the matching closing paren, tracking paren depth
        let mut depth = 1;
        let start_pos = pos;
        while pos < self.tokens.len() && depth > 0 {
            match self.tokens.get(pos) {
                Some(Token::LParen) => depth += 1,
                Some(Token::RParen) => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                pos += 1;
            }
        }
        
        if depth != 0 {
            return false;
        }
        
        // pos is now at the closing paren
        let close_paren_pos = pos;
        pos += 1;
        
        if pos >= self.tokens.len() {
            return false;
        }
        
        // Check what follows the closing paren
        // Case 1: Tuple type declaration: (type, type) name = expr;
        // Case 2: Tuple destructuring: (type name, type name) = expr;
        
        let is_destructure = matches!(self.tokens.get(pos), Some(Token::Assign));
        let is_type_decl = matches!(self.tokens.get(pos), Some(Token::Ident(_)));
        
        if !is_destructure && !is_type_decl {
            return false;
        }
        
        if is_type_decl {
            pos += 1;
            if pos >= self.tokens.len() || !matches!(self.tokens.get(pos), Some(Token::Assign)) {
                return false;
            }
        }
        
        // Now verify the content inside parens looks like a tuple
        // It should have at least one comma at depth 0
        let mut has_comma = false;
        let mut inner_depth = 0;
        for i in start_pos..close_paren_pos {
            match self.tokens.get(i) {
                Some(Token::LParen) => inner_depth += 1,
                Some(Token::RParen) => inner_depth -= 1,
                Some(Token::Comma) if inner_depth == 0 => has_comma = true,
                _ => {}
            }
        }
        
        has_comma
    }

    fn parse_tuple_decl(&mut self) -> Result<Stmt, String> {
        self.consume(Token::LParen, "expected `(`")?;
        
        // Try to parse as tuple type declaration first: (type1, type2) name = ...
        // or tuple destructuring: (type1 name1, type2 name2) = ...
        
        let first_type = self.parse_type()?;
        
        // Check what comes after the first type
        if self.check(Token::Comma) || self.check(Token::RParen) {
            // This is tuple type declaration: (type1, type2) name = ...
            let mut types = vec![first_type];
            
            while self.match_token(Token::Comma) {
                types.push(self.parse_type()?);
            }
            
            self.consume(Token::RParen, "expected `)`")?;
            let name = self.consume_ident("expected variable name")?;
            self.consume(Token::Assign, "expected `=`")?;
            let expr = self.parse_expr()?;
            self.consume_semi()?;
            
            // Create a VarDecl with tuple type
            Ok(Stmt::VarDecl(Type::Tuple(types), name, Some(expr)))
        } else if self.check(Token::Ident(String::new())) {
            // This is tuple destructuring: (type name, ...)
            let mut names = Vec::new();
            let name = self.consume_ident("expected variable name")?;
            names.push(name);
            
            while self.match_token(Token::Comma) {
                let _ty = self.parse_type()?;
                let name = self.consume_ident("expected variable name")?;
                names.push(name);
            }
            
            self.consume(Token::RParen, "expected `)`")?;
            self.consume(Token::Assign, "expected `=`")?;
            let expr = self.parse_expr()?;
            self.consume_semi()?;
            Ok(Stmt::TupleDecl(names, expr))
        } else {
            Err("expected `,` or variable name in tuple declaration".to_string())
        }
    }

    fn parse_if(&mut self) -> Result<Stmt, String> {
        self.consume(Token::LParen, "expected `(` after `if`")?;
        let cond = self.parse_expr()?;
        self.consume(Token::RParen, "expected `)`")?;
        let then_branch = self.parse_stmt_body()?;
        let else_branch = if self.match_token(Token::Else) {
            Some(self.parse_stmt_body()?)
        } else {
            None
        };
        Ok(Stmt::If(cond, then_branch, else_branch))
    }

    fn parse_while(&mut self) -> Result<Stmt, String> {
        self.consume(Token::LParen, "expected `(` after `while`")?;
        let cond = self.parse_expr()?;
        self.consume(Token::RParen, "expected `)`")?;
        let body = self.parse_stmt_body()?;
        Ok(Stmt::While(cond, body))
    }

    fn parse_for(&mut self) -> Result<Stmt, String> {
        self.consume(Token::LParen, "expected `(` after `for`")?;
        
        // Parse init statement (can be var decl, expr, or empty)
        let init = if self.check(Token::Semi) {
            self.advance(); // consume ;
            Stmt::Block(vec![]) // empty init
        } else if self.check_type() && self.peek_ahead_is_ident_after_type() {
            let stmt = self.parse_var_decl()?;
            stmt
        } else {
            let expr = self.parse_expr()?;
            if self.match_token(Token::Assign) {
                let target = expr_to_assign_target(expr)?;
                let value = self.parse_expr()?;
                self.consume(Token::Semi, "expected `;`")?;
                Stmt::Assign(target, value)
            } else {
                self.consume(Token::Semi, "expected `;`")?;
                Stmt::Expr(expr)
            }
        };
        
        // Parse condition (can be empty)
        let cond = if self.check(Token::Semi) {
            Expr::Bool(true) // empty condition = always true
        } else {
            self.parse_expr()?
        };
        self.consume(Token::Semi, "expected `;`")?;
        
        // Parse update (can be empty)
        let update = if self.check(Token::RParen) {
            Stmt::Block(vec![]) // empty update
        } else {
            let expr = self.parse_expr()?;
            if self.match_token(Token::Assign) {
                let target = expr_to_assign_target(expr)?;
                let value = self.parse_expr()?;
                Stmt::Assign(target, value)
            } else {
                Stmt::Expr(expr)
            }
        };
        
        self.consume(Token::RParen, "expected `)`")?;
        let body = self.parse_stmt_body()?;
        
        Ok(Stmt::For(Box::new(init), cond, Box::new(update), body))
    }

    fn parse_do_while(&mut self) -> Result<Stmt, String> {
        let body = self.parse_stmt_body()?;
        self.consume(Token::While, "expected `while` after `do` body")?;
        self.consume(Token::LParen, "expected `(` after `while`")?;
        let cond = self.parse_expr()?;
        self.consume(Token::RParen, "expected `)`")?;
        self.consume_semi()?;
        Ok(Stmt::DoWhile(body, cond))
    }

    fn parse_return(&mut self) -> Result<Stmt, String> {
        let expr = if self.check(Token::Semi) || self.check(Token::Newline) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.consume_semi()?;
        Ok(Stmt::Return(expr))
    }

    fn parse_var_decl(&mut self) -> Result<Stmt, String> {
        let ty = self.parse_type()?;
        let name = self.consume_ident("expected variable name")?;
        let init = if self.match_token(Token::Assign) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.consume_semi()?;
        Ok(Stmt::VarDecl(ty, name, init))
    }

    fn parse_stmt_body(&mut self) -> Result<Vec<Stmt>, String> {
        self.skip_newlines();
        if self.match_token(Token::LBrace) {
            self.parse_block()
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        let expr = self.parse_or()?;
        if self.match_token(Token::Question) {
            let then_expr = self.parse_expr()?;
            self.consume(Token::Colon, "expected `:` in ternary expression")?;
            let else_expr = self.parse_expr()?;
            Ok(Expr::Ternary(Box::new(expr), Box::new(then_expr), Box::new(else_expr)))
        } else {
            Ok(expr)
        }
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.match_token(Token::Or) {
            let right = self.parse_and()?;
            left = Expr::Binary(BinOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_equality()?;
        while self.match_token(Token::And) {
            let right = self.parse_equality()?;
            left = Expr::Binary(BinOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_relational()?;
        loop {
            if self.match_token(Token::Eq) {
                let right = self.parse_relational()?;
                left = Expr::Binary(BinOp::Eq, Box::new(left), Box::new(right));
            } else if self.match_token(Token::Ne) {
                let right = self.parse_relational()?;
                left = Expr::Binary(BinOp::Ne, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_relational(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_additive()?;
        loop {
            if self.match_token(Token::Lt) {
                let right = self.parse_additive()?;
                left = Expr::Binary(BinOp::Lt, Box::new(left), Box::new(right));
            } else if self.match_token(Token::Le) {
                let right = self.parse_additive()?;
                left = Expr::Binary(BinOp::Le, Box::new(left), Box::new(right));
            } else if self.match_token(Token::Gt) {
                let right = self.parse_additive()?;
                left = Expr::Binary(BinOp::Gt, Box::new(left), Box::new(right));
            } else if self.match_token(Token::Ge) {
                let right = self.parse_additive()?;
                left = Expr::Binary(BinOp::Ge, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplicative()?;
        loop {
            if self.match_token(Token::Plus) {
                let right = self.parse_multiplicative()?;
                left = Expr::Binary(BinOp::Add, Box::new(left), Box::new(right));
            } else if self.match_token(Token::Minus) {
                let right = self.parse_multiplicative()?;
                left = Expr::Binary(BinOp::Sub, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            if self.match_token(Token::Star) {
                let right = self.parse_unary()?;
                left = Expr::Binary(BinOp::Mul, Box::new(left), Box::new(right));
            } else if self.match_token(Token::Slash) {
                let right = self.parse_unary()?;
                left = Expr::Binary(BinOp::Div, Box::new(left), Box::new(right));
            } else if self.match_token(Token::Percent) {
                let right = self.parse_unary()?;
                left = Expr::Binary(BinOp::Rem, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.match_token(Token::Minus) {
            let operand = self.parse_unary()?;
            Ok(Expr::Unary(UnaryOp::Neg, Box::new(operand)))
        } else if self.match_token(Token::Bang) {
            let operand = self.parse_unary()?;
            Ok(Expr::Unary(UnaryOp::Not, Box::new(operand)))
        } else {
            self.parse_call_or_field()
        }
    }

    fn parse_call_or_field(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.match_token(Token::Dot) {
                // Check for tuple index access: .0, .1, etc.
                // Also handle the case where the lexer produced a float literal like 0.0
                let tuple_index = match self.peek() {
                    Some(Token::IntLit(idx)) => {
                        let idx = *idx as usize;
                        self.advance();
                        Some(idx)
                    }
                    Some(Token::FloatLit(f)) => {
                        // If we see a float like 0.0 after a dot, treat the integer part as the index
                        let idx = *f as usize;
                        self.advance();
                        Some(idx)
                    }
                    _ => None,
                };
                if let Some(idx) = tuple_index {
                    expr = Expr::TupleIndex(Box::new(expr), idx);
                    continue;
                }
                let name = self.consume_ident("expected member name after `.`")?;
                if let Expr::Var(ref target) = expr {
                    if target == "super" {
                        if self.check(Token::LParen) {
                            let args = self.parse_args()?;
                            expr = Expr::SuperCall(name, args);
                        } else {
                            expr = Expr::SuperField(name);
                        }
                        continue;
                    }
                }
                if let Expr::Var(ref class_name) = expr {
                    let is_type_name = class_name.chars().next().map_or(false, |c| c.is_ascii_uppercase());
                    if is_type_name {
                        if self.check(Token::LParen) {
                            let args = self.parse_args()?;
                            expr = Expr::Call(CallExpr {
                                target: None,
                                class_or_target: class_name.clone(),
                                method: name,
                                type_args: Vec::new(),
                                args,
                            });
                        } else {
                            expr = Expr::StaticField(class_name.clone(), name);
                        }
                        continue;
                    }
                }
                let type_args = if self.check(Token::Lt) {
                    if self.lookahead_is_type_args() {
                        self.parse_type_args()?
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                if self.check(Token::LParen) {
                    let args = self.parse_args()?;
                    expr = Expr::Call(CallExpr {
                        target: Some(Box::new(expr.clone())),
                        class_or_target: name.clone(),
                        method: name,
                        type_args,
                        args,
                    });
                } else {
                    expr = Expr::Field(Box::new(expr), name);
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn lookahead_is_type_args(&mut self) -> bool {
        // Simple heuristic: if we see `<` followed by an identifier, it's likely type args
        // This doesn't handle all cases but works for common patterns
        if !self.check(Token::Lt) {
            return false;
        }
        // Look at the next token
        let mut lookahead = self.pos + 1;
        while lookahead < self.tokens.len() {
            match &self.tokens[lookahead] {
                Token::Newline => {
                    lookahead += 1;
                    continue;
                }
                Token::Ident(_) => return true,
                _ => return false,
            }
        }
        false
    }

    fn is_class_name(&self, _name: &str) -> bool {
        false
    }

    fn finish_call(&mut self, target: Expr, method: String) -> Result<Expr, String> {
        let args = self.parse_args()?;
        Ok(Expr::Call(CallExpr {
            target: Some(Box::new(target)),
            class_or_target: method.clone(),
            method,
            type_args: Vec::new(),
            args,
        }))
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        if self.match_token(Token::Match) {
            return self.parse_match();
        }
        if self.match_token(Token::True) {
            return Ok(Expr::Bool(true));
        }
        if self.match_token(Token::False) {
            return Ok(Expr::Bool(false));
        }
        if self.match_token(Token::Null) {
            return Ok(Expr::Null);
        }
        if self.match_token(Token::Super) {
            return Ok(Expr::Var("super".to_string()));
        }
        if self.match_token(Token::New) {
            let name = self.consume_ident("expected class name after `new`")?;
            let type_args = if self.check(Token::Lt) {
                self.parse_type_args()?
            } else {
                Vec::new()
            };
            self.consume(Token::LParen, "expected `(`")?;
            self.consume(Token::RParen, "expected `)`")?;
            return Ok(Expr::New(name, type_args));
        }

        match self.peek() {
            Some(Token::IntLit(i)) => {
                let v = *i;
                self.advance();
                Ok(Expr::Int(v))
            }
            Some(Token::FloatLit(x)) => {
                let v = *x;
                self.advance();
                Ok(Expr::Float(v))
            }
            Some(Token::StringLit(s)) => {
                let v = s.clone();
                self.advance();
                
                // Check if this is an interpolated string
                if self.check(Token::InterpStart) {
                    let mut parts = vec![InterpPart::Literal(v)];
                    
                    while self.match_token(Token::InterpStart) {
                        // Parse the expression inside the interpolation
                        let expr = self.parse_expr()?;
                        parts.push(InterpPart::Expr(expr));
                        
                        // Expect InterpEnd
                        if !self.match_token(Token::InterpEnd) {
                            return Err("expected `}` to close string interpolation".to_string());
                        }
                        
                        // Check if there's another string part
                        if let Some(Token::StringLit(s)) = self.peek() {
                            parts.push(InterpPart::Literal(s.clone()));
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    
                    Ok(Expr::InterpolatedString(parts))
                } else {
                    Ok(Expr::String(v))
                }
            }
            Some(Token::Ident(name)) => {
                let name = name.clone();
                self.advance();
                if self.check(Token::LParen) {
                    let args = self.parse_args()?;
                    if name == "print" || name == "println" {
                        Ok(Expr::Call(CallExpr {
                            target: None,
                            class_or_target: "__intrinsics".to_string(),
                            method: name,
                            type_args: Vec::new(),
                            args,
                        }))
                    } else {
                        Ok(Expr::Call(CallExpr {
                            target: None,
                            class_or_target: name.clone(),
                            method: name,
                            type_args: Vec::new(),
                            args,
                        }))
                    }
                } else {
                    Ok(Expr::Var(name))
                }
            }
            Some(Token::LParen) => {
                self.advance();
                let expr = self.parse_expr()?;
                if self.match_token(Token::Comma) {
                    // This is a tuple literal
                    let mut elements = vec![expr];
                    if !self.check(Token::RParen) {
                        loop {
                            elements.push(self.parse_expr()?);
                            if !self.match_token(Token::Comma) {
                                break;
                            }
                        }
                    }
                    self.consume(Token::RParen, "expected `)`")?;
                    Ok(Expr::Tuple(elements))
                } else {
                    // This is a parenthesized expression
                    self.consume(Token::RParen, "expected `)`")?;
                    Ok(expr)
                }
            }
            Some(t) => Err(format!("unexpected token {}", t)),
            None => Err("unexpected end of input".to_string()),
        }
    }

    fn parse_match(&mut self) -> Result<Expr, String> {
        self.consume(Token::LParen, "expected `(` after `match`")?;
        let subject = self.parse_expr()?;
        self.consume(Token::RParen, "expected `)`")?;
        self.consume(Token::LBrace, "expected `{`")?;
        
        let mut arms = Vec::new();
        while !self.check(Token::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(Token::RBrace) {
                break;
            }
            arms.push(self.parse_match_arm()?);
            self.skip_newlines();
            self.match_token(Token::Comma);
            self.skip_newlines();
        }
        self.consume(Token::RBrace, "expected `}`")?;
        Ok(Expr::Match(Box::new(subject), arms))
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, String> {
        let mut patterns = vec![self.parse_pattern()?];
        while self.match_token(Token::Or) {
            patterns.push(self.parse_pattern()?);
        }
        
        let guard = if self.match_token(Token::If) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        
        self.consume(Token::FatArrow, "expected `=>`")?;
        let body = self.parse_expr()?;
        self.skip_newlines();
        
        Ok(MatchArm { patterns, guard, body })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        if self.match_token(Token::Star) {
            return Ok(Pattern::Wildcard);
        }
        if self.match_token(Token::Null) {
            return Ok(Pattern::Null);
        }
        if self.match_token(Token::True) {
            return Ok(Pattern::Bool(true));
        }
        if self.match_token(Token::False) {
            return Ok(Pattern::Bool(false));
        }
        if let Some(Token::IntLit(i)) = self.peek() {
            let v = *i;
            self.advance();
            return Ok(Pattern::Int(v));
        }
        if let Some(Token::FloatLit(x)) = self.peek() {
            let v = *x;
            self.advance();
            return Ok(Pattern::Float(v));
        }
        if let Some(Token::StringLit(s)) = self.peek() {
            let v = s.clone();
            self.advance();
            return Ok(Pattern::StringLit(v));
        }
        if let Some(Token::Ident(name)) = self.peek() {
            let name = name.clone();
            self.advance();
            if self.match_token(Token::Dot) {
                let variant = self.consume_ident("expected variant name")?;
                let args = if self.check(Token::LParen) {
                    self.consume(Token::LParen, "expected `(`")?;
                    let mut args = Vec::new();
                    if !self.check(Token::RParen) {
                        loop {
                            let arg = self.consume_ident("expected binding name")?;
                            args.push(arg);
                            if !self.match_token(Token::Comma) {
                                break;
                            }
                        }
                    }
                    self.consume(Token::RParen, "expected `)`")?;
                    args
                } else {
                    Vec::new()
                };
                return Ok(Pattern::EnumVariant(name, variant, args));
            }
            return Ok(Pattern::Binding(name));
        }
        Err(format!("expected pattern, found {}", self.peek_desc()))
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, String> {
        self.consume(Token::LParen, "expected `(`")?;
        let mut args = Vec::new();
        if !self.check(Token::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if !self.match_token(Token::Comma) {
                    break;
                }
            }
        }
        self.consume(Token::RParen, "expected `)`")?;
        Ok(args)
    }

    fn parse_type(&mut self) -> Result<Type, String> {
        match self.peek() {
            Some(Token::Void) => {
                self.advance();
                Ok(Type::Unit)
            }
            Some(Token::Int) => {
                self.advance();
                Ok(Type::Int)
            }
            Some(Token::Float) => {
                self.advance();
                Ok(Type::Float)
            }
            Some(Token::Bool) => {
                self.advance();
                Ok(Type::Bool)
            }
            Some(Token::String) => {
                self.advance();
                Ok(Type::String)
            }
            Some(Token::LParen) => {
                self.advance();
                let mut types = Vec::new();
                if !self.check(Token::RParen) {
                    loop {
                        types.push(self.parse_type()?);
                        if !self.match_token(Token::Comma) {
                            break;
                        }
                    }
                }
                self.consume(Token::RParen, "expected `)`")?;
                if types.len() == 1 {
                    Ok(types.into_iter().next().unwrap())
                } else {
                    Ok(Type::Tuple(types))
                }
            }
            Some(Token::Ident(n)) => {
                let n = n.clone();
                self.advance();
                // Check for type arguments
                if self.check(Token::Lt) {
                    let args = self.parse_type_args()?;
                    Ok(Type::Class(n, args))
                } else {
                    // Check if it's a generic parameter reference (single uppercase letter)
                    // or a class name. For now, treat all as Class.
                    Ok(Type::Class(n, Vec::new()))
                }
            }
            Some(t) => Err(format!("expected type, found {}", t)),
            None => Err("expected type".to_string()),
        }
    }

    fn parse_type_args(&mut self) -> Result<Vec<Type>, String> {
        self.consume(Token::Lt, "expected `<`")?;
        let mut args = Vec::new();
        loop {
            args.push(self.parse_type()?);
            if !self.match_token(Token::Comma) {
                break;
            }
        }
        self.consume(Token::Gt, "expected `>`")?;
        Ok(args)
    }

    fn check_type(&self) -> bool {
        matches!(
            self.peek(),
            Some(Token::Void | Token::Int | Token::Float | Token::Bool | Token::String | Token::Ident(_) | Token::LParen)
        )
    }

    fn check_type_token(&self, tok: &Token) -> bool {
        matches!(
            tok,
            Token::Void | Token::Int | Token::Float | Token::Bool | Token::String | Token::Ident(_) | Token::LParen
        )
    }

    fn peek_ahead_is_ident_after_type(&self) -> bool {
        let mut pos = self.pos;
        if pos >= self.tokens.len() {
            return false;
        }
        if !self.check_type_token(&self.tokens[pos]) {
            return false;
        }
        pos += 1;
        // Skip generic type arguments if present
        if pos < self.tokens.len() && matches!(self.tokens.get(pos), Some(Token::Lt)) {
            let mut depth = 1;
            pos += 1;
            while pos < self.tokens.len() && depth > 0 {
                match self.tokens.get(pos) {
                    Some(Token::Lt) => depth += 1,
                    Some(Token::Gt) => depth -= 1,
                    None => return false,
                    _ => {}
                }
                pos += 1;
            }
        }
        matches!(self.tokens.get(pos), Some(Token::Ident(_)))
    }

    fn consume(&mut self, expected: Token, msg: &str) -> Result<(), String> {
        if self.check_token(&expected) {
            self.advance();
            Ok(())
        } else {
            Err(format!("{} (found {})", msg, self.peek_desc()))
        }
    }

    fn consume_ident(&mut self, msg: &str) -> Result<String, String> {
        match self.peek() {
            Some(Token::Ident(s)) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            _ => Err(format!("{} (found {})", msg, self.peek_desc())),
        }
    }

    fn consume_semi(&mut self) -> Result<(), String> {
        self.skip_newlines();
        if self.check(Token::Semi) {
            self.advance();
        }
        Ok(())
    }

    fn match_token(&mut self, expected: Token) -> bool {
        if self.check_token(&expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, expected: Token) -> bool {
        self.check_token(&expected)
    }

    fn check_token(&self, expected: &Token) -> bool {
        match self.peek() {
            Some(t) => discriminant_eq(t, expected),
            None => false,
        }
    }

    fn advance(&mut self) -> Option<Token> {
        if !self.is_at_end() {
            self.pos += 1;
            Some(self.tokens[self.pos - 1].clone())
        } else {
            None
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_desc(&self) -> String {
        match self.peek() {
            Some(t) => t.to_string(),
            None => "EOF".to_string(),
        }
    }

    fn skip_newlines(&mut self) {
        while self.check(Token::Newline) {
            self.advance();
        }
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek(), Some(Token::Eof) | None)
    }
}

fn discriminant_eq(a: &Token, b: &Token) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

fn expr_to_assign_target(expr: Expr) -> Result<AssignTarget, String> {
    match expr {
        Expr::Var(name) => Ok(AssignTarget::Local(name)),
        Expr::Field(obj, name) => Ok(AssignTarget::Field(obj, name)),
        Expr::StaticField(class, name) => Ok(AssignTarget::StaticField(class, name)),
        Expr::SuperField(name) => Ok(AssignTarget::SuperField(name)),
        _ => Err("invalid assignment target".to_string()),
    }
}
