//! Syntax tree of the EXPRESS subset the IFC schemas use (ISO 10303-11): one schema with type,
//! entity, function and rule declarations. Every declaration and rule keeps its source text.

#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    pub name: String,
    pub types: Vec<TypeDecl>,
    pub entities: Vec<EntityDecl>,
    pub functions: Vec<Algorithm>,
    pub rules: Vec<RuleDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub body: TypeBody,
    pub where_rules: Vec<WhereRule>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeBody {
    Defined(TypeSpec),
    Enumeration {
        extensible: bool,
        based_on: Option<String>,
        items: Vec<String>,
    },
    Select {
        extensible: bool,
        based_on: Option<String>,
        members: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeSpec {
    Integer,
    Real(Option<Expr>),
    Number,
    Boolean,
    Logical,
    String {
        width: Option<Expr>,
        fixed: bool,
    },
    Binary {
        width: Option<Expr>,
        fixed: bool,
    },
    Named(String),
    Aggregate(Box<Aggregate>),
    /// `GENERIC [: label]`.
    Generic(Option<String>),
    /// `GENERIC_ENTITY [: label]`.
    GenericEntity(Option<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggregateKind {
    Array,
    List,
    Set,
    Bag,
    /// The general `AGGREGATE [: label]` of formal parameters.
    General,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Aggregate {
    pub kind: AggregateKind,
    /// `[lower : upper]`; `None` where the declaration gives no bounds.
    pub bounds: Option<(Bound, Bound)>,
    /// `ARRAY ... OF OPTIONAL`.
    pub optional: bool,
    pub unique: bool,
    pub label: Option<String>,
    pub element: TypeSpec,
}

/// One bound of an aggregate with its source text.
#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    pub expr: Expr,
    pub source: String,
}

/// `SUPERTYPE OF (...)` expressions.
#[derive(Clone, Debug, PartialEq)]
pub enum Supertype {
    Entity(String),
    OneOf(Vec<Supertype>),
    And(Box<Supertype>, Box<Supertype>),
    AndOr(Box<Supertype>, Box<Supertype>),
}

/// The name of an attribute, possibly re-declaring an inherited one (`SELF\Entity.Attr`).
#[derive(Clone, Debug, PartialEq)]
pub enum AttrName {
    Plain(String),
    Redeclared {
        entity: String,
        attribute: String,
        renamed: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntityDecl {
    pub name: String,
    pub is_abstract: bool,
    pub supertype_of: Option<Supertype>,
    pub subtype_of: Vec<String>,
    pub explicit: Vec<Explicit>,
    pub derived: Vec<Derived>,
    pub inverse: Vec<Inverse>,
    pub unique: Vec<UniqueRule>,
    pub where_rules: Vec<WhereRule>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Explicit {
    pub name: AttrName,
    pub optional: bool,
    pub ty: TypeSpec,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Derived {
    pub name: AttrName,
    pub ty: TypeSpec,
    pub expr: Expr,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Inverse {
    pub name: AttrName,
    /// `SET` or `BAG` with bounds; `None` for a single reference.
    pub aggregate: Option<(AggregateKind, Option<(Bound, Bound)>)>,
    pub entity: String,
    /// `FOR [entity .] attribute`.
    pub for_entity: Option<String>,
    pub for_attribute: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UniqueRule {
    pub label: Option<String>,
    /// Attribute references as written (`Name` or `SELF\Entity.Name`).
    pub attributes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WhereRule {
    pub label: Option<String>,
    pub expr: Expr,
    /// The expression as written.
    pub source: String,
}

/// A function (or procedure) with its declarations and statements.
#[derive(Clone, Debug, PartialEq)]
pub struct Algorithm {
    pub name: String,
    pub parameters: Vec<(String, TypeSpec)>,
    pub result: Option<TypeSpec>,
    pub constants: Vec<Local>,
    pub locals: Vec<Local>,
    pub body: Vec<Stmt>,
    /// From `FUNCTION` to `END_FUNCTION;`.
    pub source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Local {
    pub name: String,
    pub ty: Option<TypeSpec>,
    pub init: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleDecl {
    pub name: String,
    pub entities: Vec<String>,
    pub locals: Vec<Local>,
    pub body: Vec<Stmt>,
    pub where_rules: Vec<WhereRule>,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Null,
    Assign(Expr, Expr),
    If {
        condition: Expr,
        then: Vec<Stmt>,
        otherwise: Vec<Stmt>,
    },
    Case {
        selector: Expr,
        branches: Vec<(Vec<Expr>, Stmt)>,
        otherwise: Option<Box<Stmt>>,
    },
    Repeat {
        increment: Option<(String, Expr, Expr, Option<Expr>)>,
        while_: Option<Expr>,
        until: Option<Expr>,
        body: Vec<Stmt>,
    },
    Return(Option<Expr>),
    Escape,
    Skip,
    Compound(Vec<Stmt>),
    /// A procedure call (`INSERT(...)`) or a call written as a statement.
    Call(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Minus,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    // Relational.
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    InstanceEq,
    InstanceNe,
    In,
    Like,
    // Additive.
    Add,
    Sub,
    Or,
    Xor,
    // Multiplicative.
    Mul,
    Div,
    IntDiv,
    Mod,
    And,
    /// `||`, the complex entity instance constructor.
    Combine,
    Power,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logical {
    True,
    False,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Integer(u64),
    Real(f64),
    Str(String),
    Encoded(String),
    Bits(String),
    Logical(Logical),
    /// `?`.
    Indeterminate,
    /// `SELF`, constants such as `PI`, attributes, variables and enumeration items.
    Name(String),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    /// `e.attribute`.
    Attribute(Box<Expr>, String),
    /// `e\Entity`.
    Group(Box<Expr>, String),
    /// `e[index]` or `e[low:high]`.
    Index(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    /// `[a, b : repetition, ...]`.
    AggregateInit(Vec<(Expr, Option<Expr>)>),
    /// `{low op item op high}`, `true` for `<=`.
    Interval {
        low: Box<Expr>,
        low_inclusive: bool,
        item: Box<Expr>,
        high_inclusive: bool,
        high: Box<Expr>,
    },
    /// `QUERY(variable <* aggregate | condition)`.
    Query {
        variable: String,
        aggregate: Box<Expr>,
        condition: Box<Expr>,
    },
}
