//! Recursive-descent parser of EXPRESS (ISO 10303-11) into [`ast`](super::ast), written from the
//! grammar of the standard. It covers what one long-form schema needs: types, entities, functions,
//! rules, statements and expressions. Interface specifications (`USE FROM`, `REFERENCE FROM`),
//! procedures, subtype constraints and nested declarations are reported as unsupported.
//!
//! Input size and nesting are bounded; errors carry the line and column.

use super::ast::{
    Aggregate, AggregateKind, Algorithm, AttrName, BinaryOp, Bound, Derived, EntityDecl, Explicit,
    Expr, Inverse, Local, Logical, RuleDecl, Schema, Stmt, Supertype, TypeBody, TypeDecl, TypeSpec,
    UnaryOp, UniqueRule, WhereRule,
};
use super::lexer::{Kind, Pos, SyntaxError, Token, tokenize};

/// Largest accepted source (the published IFC schemas are below 0.5 MiB).
pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
/// Deepest nesting of expressions, statements and types.
pub const MAX_DEPTH: usize = 128;

type Result<T> = std::result::Result<T, SyntaxError>;

/// Parses the text of one EXPRESS schema.
pub fn parse(source: &str) -> Result<Schema> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(SyntaxError {
            pos: Pos { line: 1, column: 1 },
            message: format!("source larger than {MAX_SOURCE_BYTES} bytes"),
        });
    }
    let mut parser = Parser {
        source,
        tokens: tokenize(source)?,
        at: 0,
        depth: 0,
    };
    parser.schema()
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    at: usize,
    depth: usize,
}

/// Words that end an entity section.
const ENTITY_SECTIONS: &[&str] = &["DERIVE", "INVERSE", "UNIQUE", "WHERE", "END_ENTITY"];

impl Parser<'_> {
    // ----- Token helpers -----

    fn peek(&self) -> &Token {
        self.peek_at(0)
    }

    fn peek_at(&self, ahead: usize) -> &Token {
        let last = self.tokens.len().saturating_sub(1);
        // `tokenize` always ends with `End`, so `last` exists.
        &self.tokens[(self.at + ahead).min(last)]
    }

    fn advance(&mut self) -> Token {
        let token = self.peek().clone();
        if token.kind != Kind::End {
            self.at += 1;
        }
        token
    }

    fn is(&self, word: &str) -> bool {
        self.peek().is(word)
    }

    fn is_any(&self, words: &[&str]) -> bool {
        words.iter().any(|w| self.is(w))
    }

    fn eat(&mut self, word: &str) -> bool {
        let found = self.is(word);
        if found {
            self.at += 1;
        }
        found
    }

    fn eat_sym(&mut self, sym: &str) -> bool {
        let found = self.peek().is_sym(sym);
        if found {
            self.at += 1;
        }
        found
    }

    fn expect(&mut self, word: &str) -> Result<()> {
        if self.eat(word) {
            Ok(())
        } else {
            Err(self.unexpected(word))
        }
    }

    fn expect_sym(&mut self, sym: &str) -> Result<()> {
        if self.eat_sym(sym) {
            Ok(())
        } else {
            Err(self.unexpected(&format!("'{sym}'")))
        }
    }

    fn ident(&mut self) -> Result<String> {
        match &self.peek().kind {
            Kind::Word(w) => {
                let w = w.clone();
                self.at += 1;
                Ok(w)
            }
            _ => Err(self.unexpected("a name")),
        }
    }

    fn error(&self, message: impl Into<String>) -> SyntaxError {
        SyntaxError {
            pos: self.peek().pos,
            message: message.into(),
        }
    }

    fn unexpected(&self, wanted: &str) -> SyntaxError {
        let found = match &self.peek().kind {
            Kind::Word(w) => format!("'{w}'"),
            Kind::Sym(s) => format!("'{s}'"),
            Kind::Integer(_) | Kind::Real(_) => "a number".to_owned(),
            Kind::Str(_) | Kind::Encoded(_) => "a string".to_owned(),
            Kind::Binary(_) => "a binary literal".to_owned(),
            Kind::End => "the end of the input".to_owned(),
        };
        self.error(format!("expected {wanted}, found {found}"))
    }

    /// Source text from the token at `from` to the last consumed token.
    fn text_since(&self, from: usize) -> String {
        let start = self.tokens.get(from).map_or(0, |t| t.start);
        let end = self.at.checked_sub(1).and_then(|i| self.tokens.get(i));
        let end = end.map_or(start, |t| t.end).max(start);
        self.source.get(start..end).unwrap_or_default().to_owned()
    }

    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            Err(self.error(format!("nesting deeper than {MAX_DEPTH}")))
        } else {
            Ok(())
        }
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// `name {, name}`.
    fn ident_list(&mut self) -> Result<Vec<String>> {
        let mut names = vec![self.ident()?];
        while self.eat_sym(",") {
            names.push(self.ident()?);
        }
        Ok(names)
    }

    /// `( name {, name} )`.
    fn paren_ident_list(&mut self) -> Result<Vec<String>> {
        self.expect_sym("(")?;
        let names = self.ident_list()?;
        self.expect_sym(")")?;
        Ok(names)
    }

    // ----- Schema and declarations -----

    fn schema(&mut self) -> Result<Schema> {
        self.expect("SCHEMA")?;
        let name = self.ident()?;
        if matches!(self.peek().kind, Kind::Str(_)) {
            self.advance(); // schema version identifier
        }
        self.expect_sym(";")?;
        let mut schema = Schema {
            name,
            types: Vec::new(),
            entities: Vec::new(),
            functions: Vec::new(),
            rules: Vec::new(),
        };
        loop {
            if self.eat("END_SCHEMA") {
                self.expect_sym(";")?;
                break;
            } else if self.is("TYPE") {
                schema.types.push(self.type_decl()?);
            } else if self.is("ENTITY") {
                schema.entities.push(self.entity()?);
            } else if self.is("FUNCTION") {
                schema.functions.push(self.function()?);
            } else if self.is("RULE") {
                schema.rules.push(self.rule()?);
            } else if self.is_any(&[
                "USE",
                "REFERENCE",
                "PROCEDURE",
                "SUBTYPE_CONSTRAINT",
                "CONSTANT",
            ]) {
                return Err(self.error("this EXPRESS declaration is not supported"));
            } else {
                return Err(self.unexpected("a declaration or END_SCHEMA"));
            }
        }
        if self.peek().kind != Kind::End {
            return Err(self.unexpected("the end of the input after END_SCHEMA"));
        }
        Ok(schema)
    }

    fn type_decl(&mut self) -> Result<TypeDecl> {
        self.expect("TYPE")?;
        let name = self.ident()?;
        self.expect_sym("=")?;
        let extensible = self.eat("EXTENSIBLE");
        let generic_entity = self.eat("GENERIC_ENTITY");
        let body = if self.eat("SELECT") {
            let (based_on, members) = self.select_or_enum_list()?;
            TypeBody::Select {
                extensible,
                based_on,
                members,
            }
        } else if generic_entity {
            return Err(self.unexpected("SELECT"));
        } else if self.eat("ENUMERATION") {
            let (based_on, items) = if self.eat("OF") {
                (None, self.paren_ident_list()?)
            } else {
                self.select_or_enum_list()?
            };
            TypeBody::Enumeration {
                extensible,
                based_on,
                items,
            }
        } else if extensible {
            return Err(self.unexpected("SELECT or ENUMERATION"));
        } else {
            TypeBody::Defined(self.type_spec()?)
        };
        self.expect_sym(";")?;
        let where_rules = self.where_clause()?;
        self.expect("END_TYPE")?;
        self.expect_sym(";")?;
        Ok(TypeDecl {
            name,
            body,
            where_rules,
        })
    }

    /// `( list )` or `BASED_ON name [WITH ( list )]`, or nothing (an empty extensible type).
    fn select_or_enum_list(&mut self) -> Result<(Option<String>, Vec<String>)> {
        if self.eat("BASED_ON") {
            let base = self.ident()?;
            let items = if self.eat("WITH") {
                self.paren_ident_list()?
            } else {
                Vec::new()
            };
            Ok((Some(base), items))
        } else if self.peek().is_sym("(") {
            Ok((None, self.paren_ident_list()?))
        } else {
            Ok((None, Vec::new()))
        }
    }

    fn type_spec(&mut self) -> Result<TypeSpec> {
        self.enter()?;
        let spec = self.type_spec_inner();
        self.leave();
        spec
    }

    fn type_spec_inner(&mut self) -> Result<TypeSpec> {
        let word = self.ident()?.to_ascii_uppercase();
        Ok(match word.as_str() {
            "INTEGER" => TypeSpec::Integer,
            "NUMBER" => TypeSpec::Number,
            "BOOLEAN" => TypeSpec::Boolean,
            "LOGICAL" => TypeSpec::Logical,
            "REAL" => TypeSpec::Real(self.width()?),
            "STRING" | "BINARY" => {
                let width = self.width()?;
                let fixed = width.is_some() && self.eat("FIXED");
                if word == "STRING" {
                    TypeSpec::String { width, fixed }
                } else {
                    TypeSpec::Binary { width, fixed }
                }
            }
            "ARRAY" | "LIST" | "SET" | "BAG" | "AGGREGATE" => self.aggregate(&word)?,
            "GENERIC" => TypeSpec::Generic(self.type_label()?),
            "GENERIC_ENTITY" => TypeSpec::GenericEntity(self.type_label()?),
            _ => {
                self.at -= 1;
                TypeSpec::Named(self.ident()?)
            }
        })
    }

    /// `( width )` after REAL, STRING or BINARY.
    fn width(&mut self) -> Result<Option<Expr>> {
        if self.eat_sym("(") {
            let width = self.expr()?;
            self.expect_sym(")")?;
            Ok(Some(width))
        } else {
            Ok(None)
        }
    }

    fn type_label(&mut self) -> Result<Option<String>> {
        if self.eat_sym(":") {
            Ok(Some(self.ident()?))
        } else {
            Ok(None)
        }
    }

    fn aggregate(&mut self, word: &str) -> Result<TypeSpec> {
        let kind = match word {
            "ARRAY" => AggregateKind::Array,
            "LIST" => AggregateKind::List,
            "SET" => AggregateKind::Set,
            "BAG" => AggregateKind::Bag,
            _ => AggregateKind::General,
        };
        let label = if kind == AggregateKind::General {
            self.type_label()?
        } else {
            None
        };
        let bounds = if kind == AggregateKind::General {
            None
        } else {
            self.bounds()?
        };
        self.expect("OF")?;
        let optional = kind == AggregateKind::Array && self.eat("OPTIONAL");
        let unique =
            matches!(kind, AggregateKind::Array | AggregateKind::List) && self.eat("UNIQUE");
        let element = self.type_spec()?;
        Ok(TypeSpec::Aggregate(Box::new(Aggregate {
            kind,
            bounds,
            optional,
            unique,
            label,
            element,
        })))
    }

    /// `[ lower : upper ]`, if present.
    fn bounds(&mut self) -> Result<Option<(Bound, Bound)>> {
        if !self.eat_sym("[") {
            return Ok(None);
        }
        let lower = self.bound()?;
        self.expect_sym(":")?;
        let upper = self.bound()?;
        self.expect_sym("]")?;
        Ok(Some((lower, upper)))
    }

    fn bound(&mut self) -> Result<Bound> {
        let from = self.at;
        let expr = self.expr()?;
        Ok(Bound {
            expr,
            source: self.text_since(from),
        })
    }

    // ----- Entities -----

    fn entity(&mut self) -> Result<EntityDecl> {
        self.expect("ENTITY")?;
        let name = self.ident()?;
        let is_abstract = self.eat("ABSTRACT");
        let mut supertype_of = None;
        if self.eat("SUPERTYPE") {
            if self.eat("OF") {
                self.expect_sym("(")?;
                supertype_of = Some(self.supertype_expr()?);
                self.expect_sym(")")?;
            } else if !is_abstract {
                return Err(self.unexpected("OF"));
            }
        }
        let subtype_of = if self.eat("SUBTYPE") {
            self.expect("OF")?;
            self.paren_ident_list()?
        } else {
            Vec::new()
        };
        self.expect_sym(";")?;
        let mut entity = EntityDecl {
            name,
            is_abstract,
            supertype_of,
            subtype_of,
            explicit: Vec::new(),
            derived: Vec::new(),
            inverse: Vec::new(),
            unique: Vec::new(),
            where_rules: Vec::new(),
        };
        while !self.is_any(ENTITY_SECTIONS) {
            self.explicit(&mut entity.explicit)?;
        }
        if self.eat("DERIVE") {
            while !self.is_any(ENTITY_SECTIONS) {
                entity.derived.push(self.derived()?);
            }
        }
        if self.eat("INVERSE") {
            while !self.is_any(ENTITY_SECTIONS) {
                entity.inverse.push(self.inverse()?);
            }
        }
        if self.eat("UNIQUE") {
            while !self.is_any(ENTITY_SECTIONS) {
                entity.unique.push(self.unique()?);
            }
        }
        entity.where_rules = self.where_clause()?;
        self.expect("END_ENTITY")?;
        self.expect_sym(";")?;
        Ok(entity)
    }

    /// `ONEOF (...)`, `AND`, `ANDOR` and parentheses over entity names.
    fn supertype_expr(&mut self) -> Result<Supertype> {
        self.enter()?;
        let mut left = self.supertype_factor()?;
        while self.eat("ANDOR") {
            let right = self.supertype_factor()?;
            left = Supertype::AndOr(Box::new(left), Box::new(right));
        }
        self.leave();
        Ok(left)
    }

    fn supertype_factor(&mut self) -> Result<Supertype> {
        let mut left = self.supertype_term()?;
        while self.eat("AND") {
            let right = self.supertype_term()?;
            left = Supertype::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn supertype_term(&mut self) -> Result<Supertype> {
        if self.eat("ONEOF") {
            self.expect_sym("(")?;
            let mut items = vec![self.supertype_expr()?];
            while self.eat_sym(",") {
                items.push(self.supertype_expr()?);
            }
            self.expect_sym(")")?;
            Ok(Supertype::OneOf(items))
        } else if self.eat_sym("(") {
            let inner = self.supertype_expr()?;
            self.expect_sym(")")?;
            Ok(inner)
        } else {
            Ok(Supertype::Entity(self.ident()?))
        }
    }

    /// `name` or `SELF \ entity . attribute [RENAMED name]`.
    fn attr_name(&mut self) -> Result<AttrName> {
        if self.eat("SELF") {
            self.expect_sym("\\")?;
            let entity = self.ident()?;
            self.expect_sym(".")?;
            let attribute = self.ident()?;
            let renamed = if self.eat("RENAMED") {
                Some(self.ident()?)
            } else {
                None
            };
            Ok(AttrName::Redeclared {
                entity,
                attribute,
                renamed,
            })
        } else {
            Ok(AttrName::Plain(self.ident()?))
        }
    }

    /// `name {, name} : [OPTIONAL] type ;`
    fn explicit(&mut self, out: &mut Vec<Explicit>) -> Result<()> {
        let mut names = vec![self.attr_name()?];
        while self.eat_sym(",") {
            names.push(self.attr_name()?);
        }
        self.expect_sym(":")?;
        let optional = self.eat("OPTIONAL");
        let ty = self.type_spec()?;
        self.expect_sym(";")?;
        out.extend(names.into_iter().map(|name| Explicit {
            name,
            optional,
            ty: ty.clone(),
        }));
        Ok(())
    }

    /// `name : type := expression ;`
    fn derived(&mut self) -> Result<Derived> {
        let name = self.attr_name()?;
        self.expect_sym(":")?;
        let ty = self.type_spec()?;
        self.expect_sym(":=")?;
        let from = self.at;
        let expr = self.expr()?;
        let source = self.text_since(from);
        self.expect_sym(";")?;
        Ok(Derived {
            name,
            ty,
            expr,
            source,
        })
    }

    /// `name : [SET|BAG [bounds] OF] entity FOR [entity .] attribute ;`
    fn inverse(&mut self) -> Result<Inverse> {
        let name = self.attr_name()?;
        self.expect_sym(":")?;
        let aggregate = if self.eat("SET") {
            Some((AggregateKind::Set, self.bounds_of()?))
        } else if self.eat("BAG") {
            Some((AggregateKind::Bag, self.bounds_of()?))
        } else {
            None
        };
        let entity = self.ident()?;
        self.expect("FOR")?;
        let first = self.ident()?;
        let (for_entity, for_attribute) = if self.eat_sym(".") {
            (Some(first), self.ident()?)
        } else {
            (None, first)
        };
        self.expect_sym(";")?;
        Ok(Inverse {
            name,
            aggregate,
            entity,
            for_entity,
            for_attribute,
        })
    }

    fn bounds_of(&mut self) -> Result<Option<(Bound, Bound)>> {
        let bounds = self.bounds()?;
        self.expect("OF")?;
        Ok(bounds)
    }

    /// `[label :] attribute {, attribute} ;`
    fn unique(&mut self) -> Result<UniqueRule> {
        let label = self.rule_label()?;
        let mut attributes = Vec::new();
        loop {
            let from = self.at;
            self.attr_name()?;
            attributes.push(self.text_since(from));
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(";")?;
        Ok(UniqueRule { label, attributes })
    }

    /// A rule label: a name followed by a single `:`.
    fn rule_label(&mut self) -> Result<Option<String>> {
        if matches!(self.peek().kind, Kind::Word(_)) && self.peek_at(1).is_sym(":") {
            let label = self.ident()?;
            self.at += 1;
            Ok(Some(label))
        } else {
            Ok(None)
        }
    }

    /// `[WHERE {[label :] expression ;}]`
    fn where_clause(&mut self) -> Result<Vec<WhereRule>> {
        let mut rules = Vec::new();
        if !self.eat("WHERE") {
            return Ok(rules);
        }
        while !self.is_any(&["END_ENTITY", "END_TYPE", "END_RULE"]) {
            let label = self.rule_label()?;
            let from = self.at;
            let expr = self.expr()?;
            let source = self.text_since(from);
            self.expect_sym(";")?;
            rules.push(WhereRule {
                label,
                expr,
                source,
            });
        }
        Ok(rules)
    }

    // ----- Functions and rules -----

    fn function(&mut self) -> Result<Algorithm> {
        let from = self.at;
        self.expect("FUNCTION")?;
        let name = self.ident()?;
        let mut parameters = Vec::new();
        if self.eat_sym("(") {
            loop {
                let names = self.ident_list()?;
                self.expect_sym(":")?;
                let ty = self.type_spec()?;
                parameters.extend(names.into_iter().map(|n| (n, ty.clone())));
                if !self.eat_sym(";") {
                    break;
                }
            }
            self.expect_sym(")")?;
        }
        self.expect_sym(":")?;
        let result = Some(self.type_spec()?);
        self.expect_sym(";")?;
        let (constants, locals) = self.algorithm_head()?;
        let body = self.statements(&["END_FUNCTION"])?;
        self.expect("END_FUNCTION")?;
        self.expect_sym(";")?;
        Ok(Algorithm {
            name,
            parameters,
            result,
            constants,
            locals,
            body,
            source: self.text_since(from),
        })
    }

    /// `CONSTANT ... END_CONSTANT;` and `LOCAL ... END_LOCAL;` in any order.
    fn algorithm_head(&mut self) -> Result<(Vec<Local>, Vec<Local>)> {
        let mut constants = Vec::new();
        let mut locals = Vec::new();
        loop {
            if self.eat("CONSTANT") {
                self.locals(&mut constants, "END_CONSTANT")?;
            } else if self.eat("LOCAL") {
                self.locals(&mut locals, "END_LOCAL")?;
            } else if self.is_any(&["FUNCTION", "PROCEDURE", "TYPE", "ENTITY", "RULE"]) {
                return Err(self.error("nested declarations are not supported"));
            } else {
                return Ok((constants, locals));
            }
        }
    }

    /// `name {, name} : type [:= expression] ;` up to `end`.
    fn locals(&mut self, out: &mut Vec<Local>, end: &str) -> Result<()> {
        while !self.eat(end) {
            let names = self.ident_list()?;
            self.expect_sym(":")?;
            let ty = self.type_spec()?;
            let init = if self.eat_sym(":=") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect_sym(";")?;
            out.extend(names.into_iter().map(|name| Local {
                name,
                ty: Some(ty.clone()),
                init: init.clone(),
            }));
        }
        self.expect_sym(";")
    }

    fn rule(&mut self) -> Result<RuleDecl> {
        let from = self.at;
        self.expect("RULE")?;
        let name = self.ident()?;
        self.expect("FOR")?;
        let entities = self.paren_ident_list()?;
        self.expect_sym(";")?;
        let (_, locals) = self.algorithm_head()?;
        let body = self.statements(&["WHERE", "END_RULE"])?;
        let where_rules = self.where_clause()?;
        self.expect("END_RULE")?;
        self.expect_sym(";")?;
        Ok(RuleDecl {
            name,
            entities,
            locals,
            body,
            where_rules,
            source: self.text_since(from),
        })
    }

    // ----- Statements -----

    fn statements(&mut self, ends: &[&str]) -> Result<Vec<Stmt>> {
        let mut out = Vec::new();
        while !self.is_any(ends) {
            if self.peek().kind == Kind::End {
                return Err(self.unexpected(ends.first().copied().unwrap_or("a statement")));
            }
            out.push(self.statement()?);
        }
        Ok(out)
    }

    fn statement(&mut self) -> Result<Stmt> {
        self.enter()?;
        let stmt = self.statement_inner();
        self.leave();
        stmt
    }

    fn statement_inner(&mut self) -> Result<Stmt> {
        if self.eat_sym(";") {
            return Ok(Stmt::Null);
        }
        if self.is("IF") {
            return self.if_stmt();
        }
        if self.is("CASE") {
            return self.case_stmt();
        }
        if self.is("REPEAT") {
            return self.repeat_stmt();
        }
        let stmt = if self.eat("BEGIN") {
            let body = self.statements(&["END"])?;
            self.expect("END")?;
            Stmt::Compound(body)
        } else if self.eat("RETURN") {
            Stmt::Return(if self.peek().is_sym(";") {
                None
            } else {
                Some(self.expr()?)
            })
        } else if self.eat("ESCAPE") {
            Stmt::Escape
        } else if self.eat("SKIP") {
            Stmt::Skip
        } else if self.is("ALIAS") {
            return Err(self.error("ALIAS statements are not supported"));
        } else {
            let target = self.simple_factor()?;
            if self.eat_sym(":=") {
                Stmt::Assign(target, self.expr()?)
            } else {
                Stmt::Call(target)
            }
        };
        self.expect_sym(";")?;
        Ok(stmt)
    }

    fn if_stmt(&mut self) -> Result<Stmt> {
        self.expect("IF")?;
        let condition = self.expr()?;
        self.expect("THEN")?;
        let then = self.statements(&["ELSE", "END_IF"])?;
        let otherwise = if self.eat("ELSE") {
            self.statements(&["END_IF"])?
        } else {
            Vec::new()
        };
        self.expect("END_IF")?;
        self.expect_sym(";")?;
        Ok(Stmt::If {
            condition,
            then,
            otherwise,
        })
    }

    fn case_stmt(&mut self) -> Result<Stmt> {
        self.expect("CASE")?;
        let selector = self.expr()?;
        self.expect("OF")?;
        let mut branches = Vec::new();
        let mut otherwise = None;
        while !self.eat("END_CASE") {
            if self.eat("OTHERWISE") {
                self.expect_sym(":")?;
                otherwise = Some(Box::new(self.statement()?));
                continue;
            }
            let mut labels = vec![self.expr()?];
            while self.eat_sym(",") {
                labels.push(self.expr()?);
            }
            self.expect_sym(":")?;
            branches.push((labels, self.statement()?));
        }
        self.expect_sym(";")?;
        Ok(Stmt::Case {
            selector,
            branches,
            otherwise,
        })
    }

    fn repeat_stmt(&mut self) -> Result<Stmt> {
        self.expect("REPEAT")?;
        let increment = if matches!(self.peek().kind, Kind::Word(_)) && self.peek_at(1).is_sym(":=")
        {
            let variable = self.ident()?;
            self.expect_sym(":=")?;
            let from = self.expr()?;
            self.expect("TO")?;
            let to = self.expr()?;
            let by = if self.eat("BY") {
                Some(self.expr()?)
            } else {
                None
            };
            Some((variable, from, to, by))
        } else {
            None
        };
        let while_ = if self.eat("WHILE") {
            Some(self.expr()?)
        } else {
            None
        };
        let until = if self.eat("UNTIL") {
            Some(self.expr()?)
        } else {
            None
        };
        self.expect_sym(";")?;
        let body = self.statements(&["END_REPEAT"])?;
        self.expect("END_REPEAT")?;
        self.expect_sym(";")?;
        Ok(Stmt::Repeat {
            increment,
            while_,
            until,
            body,
        })
    }

    // ----- Expressions -----

    /// `simple_expression [rel_op simple_expression]`.
    pub(super) fn expr(&mut self) -> Result<Expr> {
        self.enter()?;
        let left = self.simple_expr()?;
        let op = match &self.peek().kind {
            Kind::Sym("=") => Some(BinaryOp::Eq),
            Kind::Sym("<>") => Some(BinaryOp::Ne),
            Kind::Sym("<") => Some(BinaryOp::Lt),
            Kind::Sym(">") => Some(BinaryOp::Gt),
            Kind::Sym("<=") => Some(BinaryOp::Le),
            Kind::Sym(">=") => Some(BinaryOp::Ge),
            Kind::Sym(":=:") => Some(BinaryOp::InstanceEq),
            Kind::Sym(":<>:") => Some(BinaryOp::InstanceNe),
            _ if self.is("IN") => Some(BinaryOp::In),
            _ if self.is("LIKE") => Some(BinaryOp::Like),
            _ => None,
        };
        let result = match op {
            Some(op) => {
                self.at += 1;
                let right = self.simple_expr()?;
                Expr::Binary(op, Box::new(left), Box::new(right))
            }
            None => left,
        };
        self.leave();
        Ok(result)
    }

    fn simple_expr(&mut self) -> Result<Expr> {
        let mut left = self.term()?;
        loop {
            let op = match &self.peek().kind {
                Kind::Sym("+") => BinaryOp::Add,
                Kind::Sym("-") => BinaryOp::Sub,
                _ if self.is("OR") => BinaryOp::Or,
                _ if self.is("XOR") => BinaryOp::Xor,
                _ => return Ok(left),
            };
            self.at += 1;
            let right = self.term()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn term(&mut self) -> Result<Expr> {
        let mut left = self.factor()?;
        loop {
            let op = match &self.peek().kind {
                Kind::Sym("*") => BinaryOp::Mul,
                Kind::Sym("/") => BinaryOp::Div,
                Kind::Sym("||") => BinaryOp::Combine,
                _ if self.is("DIV") => BinaryOp::IntDiv,
                _ if self.is("MOD") => BinaryOp::Mod,
                _ if self.is("AND") => BinaryOp::And,
                _ => return Ok(left),
            };
            self.at += 1;
            let right = self.factor()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn factor(&mut self) -> Result<Expr> {
        let base = self.simple_factor()?;
        if self.eat_sym("**") {
            let exponent = self.simple_factor()?;
            Ok(Expr::Binary(
                BinaryOp::Power,
                Box::new(base),
                Box::new(exponent),
            ))
        } else {
            Ok(base)
        }
    }

    fn simple_factor(&mut self) -> Result<Expr> {
        self.enter()?;
        let result = self.simple_factor_inner();
        self.leave();
        result
    }

    fn simple_factor_inner(&mut self) -> Result<Expr> {
        let unary = match &self.peek().kind {
            Kind::Sym("+") => Some(UnaryOp::Plus),
            Kind::Sym("-") => Some(UnaryOp::Minus),
            _ if self.is("NOT") => Some(UnaryOp::Not),
            _ => None,
        };
        if let Some(op) = unary {
            self.at += 1;
            return Ok(Expr::Unary(op, Box::new(self.simple_factor()?)));
        }
        let primary = if self.eat_sym("(") {
            let inner = self.expr()?;
            self.expect_sym(")")?;
            inner
        } else if self.peek().is_sym("[") {
            self.aggregate_init()?
        } else if self.peek().is_sym("{") {
            return self.interval();
        } else if self.is("QUERY") {
            return self.query();
        } else {
            self.primary()?
        };
        self.qualifiers(primary)
    }

    /// A literal, a name or a call.
    fn primary(&mut self) -> Result<Expr> {
        let token = self.advance();
        Ok(match token.kind {
            Kind::Integer(v) => Expr::Integer(v),
            Kind::Real(v) => Expr::Real(v),
            Kind::Str(s) => Expr::Str(s),
            Kind::Encoded(s) => Expr::Encoded(s),
            Kind::Binary(s) => Expr::Bits(s),
            Kind::Sym("?") => Expr::Indeterminate,
            Kind::Word(w) if w.eq_ignore_ascii_case("TRUE") => Expr::Logical(Logical::True),
            Kind::Word(w) if w.eq_ignore_ascii_case("FALSE") => Expr::Logical(Logical::False),
            Kind::Word(w) if w.eq_ignore_ascii_case("UNKNOWN") => Expr::Logical(Logical::Unknown),
            Kind::Word(w) => {
                if self.eat_sym("(") {
                    let mut args = Vec::new();
                    if !self.eat_sym(")") {
                        args.push(self.expr()?);
                        while self.eat_sym(",") {
                            args.push(self.expr()?);
                        }
                        self.expect_sym(")")?;
                    }
                    Expr::Call(w, args)
                } else {
                    Expr::Name(w)
                }
            }
            _ => {
                self.at -= 1;
                return Err(self.unexpected("an expression"));
            }
        })
    }

    /// `.attribute`, `\entity` and `[index]` or `[low : high]` after a primary.
    fn qualifiers(&mut self, mut expr: Expr) -> Result<Expr> {
        loop {
            if self.eat_sym(".") {
                expr = Expr::Attribute(Box::new(expr), self.ident()?);
            } else if self.eat_sym("\\") {
                expr = Expr::Group(Box::new(expr), self.ident()?);
            } else if self.eat_sym("[") {
                let index = self.expr()?;
                let high = if self.eat_sym(":") {
                    Some(Box::new(self.expr()?))
                } else {
                    None
                };
                self.expect_sym("]")?;
                expr = Expr::Index(Box::new(expr), Box::new(index), high);
            } else {
                return Ok(expr);
            }
        }
    }

    /// `[ element [: repetition] {, ...} ]`.
    fn aggregate_init(&mut self) -> Result<Expr> {
        self.expect_sym("[")?;
        let mut elements = Vec::new();
        if !self.eat_sym("]") {
            loop {
                let element = self.expr()?;
                let repetition = if self.eat_sym(":") {
                    Some(self.expr()?)
                } else {
                    None
                };
                elements.push((element, repetition));
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym("]")?;
        }
        Ok(Expr::AggregateInit(elements))
    }

    /// `{ low (<|<=) item (<|<=) high }`.
    fn interval(&mut self) -> Result<Expr> {
        self.expect_sym("{")?;
        let low = self.simple_expr()?;
        let low_inclusive = self.interval_op()?;
        let item = self.simple_expr()?;
        let high_inclusive = self.interval_op()?;
        let high = self.simple_expr()?;
        self.expect_sym("}")?;
        Ok(Expr::Interval {
            low: Box::new(low),
            low_inclusive,
            item: Box::new(item),
            high_inclusive,
            high: Box::new(high),
        })
    }

    fn interval_op(&mut self) -> Result<bool> {
        if self.eat_sym("<=") {
            Ok(true)
        } else if self.eat_sym("<") {
            Ok(false)
        } else {
            Err(self.unexpected("'<' or '<='"))
        }
    }

    /// `QUERY ( variable <* aggregate | condition )`.
    fn query(&mut self) -> Result<Expr> {
        self.expect("QUERY")?;
        self.expect_sym("(")?;
        let variable = self.ident()?;
        self.expect_sym("<*")?;
        let aggregate = self.simple_expr()?;
        self.expect_sym("|")?;
        let condition = self.expr()?;
        self.expect_sym(")")?;
        Ok(Expr::Query {
            variable,
            aggregate: Box::new(aggregate),
            condition: Box::new(condition),
        })
    }
}

#[cfg(test)]
mod tests;
