//! The parsed form of the EXPRESS expressions, statements and algorithms of a schema: WHERE rules,
//! derived attributes, functions and global rules. The generator resolves every name, so a tree
//! refers to variables by slot, to functions and entities by table index and to attributes by
//! name; the rule interpreter evaluates the trees.

/// A truth value of EXPRESS: `FALSE < UNKNOWN < TRUE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Logical {
    /// `FALSE`.
    False,
    /// `UNKNOWN`.
    Unknown,
    /// `TRUE`.
    True,
}

/// A unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `+`.
    Plus,
    /// `-`.
    Minus,
    /// `NOT`.
    Not,
}

/// A binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `=` (value equality).
    Eq,
    /// `<>`.
    Ne,
    /// `<`.
    Lt,
    /// `>`.
    Gt,
    /// `<=`.
    Le,
    /// `>=`.
    Ge,
    /// `:=:` (instance equality).
    InstanceEq,
    /// `:<>:`.
    InstanceNe,
    /// `IN`.
    In,
    /// `LIKE`.
    Like,
    /// `+`: addition, string concatenation or union.
    Add,
    /// `-`: subtraction or difference.
    Sub,
    /// `OR`.
    Or,
    /// `XOR`.
    Xor,
    /// `*`: multiplication or intersection.
    Mul,
    /// `/`: real division.
    Div,
    /// `DIV`: integer division.
    IntDiv,
    /// `MOD`.
    Mod,
    /// `AND`.
    And,
    /// `||`: the complex entity instance constructor.
    Combine,
    /// `**`.
    Power,
}

/// A built-in function of EXPRESS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// `ABS`.
    Abs,
    /// `ACOS`.
    Acos,
    /// `ASIN`.
    Asin,
    /// `ATAN(V1, V2)`.
    Atan,
    /// `BLENGTH`.
    Blength,
    /// `COS`.
    Cos,
    /// `EXISTS`.
    Exists,
    /// `EXP`.
    Exp,
    /// `HIINDEX`.
    Hiindex,
    /// `LENGTH`.
    Length,
    /// `LOINDEX`.
    Loindex,
    /// `LOG`.
    Log,
    /// `LOG2`.
    Log2,
    /// `LOG10`.
    Log10,
    /// `NVL`.
    Nvl,
    /// `ODD`.
    Odd,
    /// `SIN`.
    Sin,
    /// `SIZEOF`.
    Sizeof,
    /// `SQRT`.
    Sqrt,
    /// `TAN`.
    Tan,
    /// `TYPEOF`.
    Typeof,
    /// `USEDIN`.
    Usedin,
    /// `VALUE`.
    Value,
}

/// An expression.
#[derive(Debug, PartialEq)]
pub enum Expr {
    /// An integer literal.
    Integer(i64),
    /// A real literal; `PI` and `CONST_E` are written as their values.
    Real(f64),
    /// A string literal.
    String(&'static str),
    /// A binary literal, the bits as written.
    Bits(&'static str),
    /// `TRUE`, `FALSE` or `UNKNOWN`.
    Logical(Logical),
    /// `?`.
    Indeterminate,
    /// `SELF`: the instance of an entity rule or derived attribute, the value of a type rule.
    SelfValue,
    /// A parameter, local variable, query or loop variable, by slot.
    Variable(u16),
    /// The instances of an entity a global rule ranges over.
    Population(u16),
    /// An enumeration item, upper case.
    EnumItem(&'static str),
    /// `e.attribute`; a group qualifier `e\entity` is left out, the attribute is found by name.
    Attribute(&'static Expr, &'static str),
    /// `e[index]`, or `e[low : high]`.
    Index(&'static Expr, &'static Expr, Option<&'static Expr>),
    /// A unary operation.
    Unary(UnaryOp, &'static Expr),
    /// A binary operation.
    Binary(BinaryOp, &'static Expr, &'static Expr),
    /// A built-in function call.
    Builtin(Builtin, &'static [Expr]),
    /// A call of a schema function, by index into `SchemaTables::functions`.
    Call(u16, &'static [Expr]),
    /// An entity constructor: the entity index and its own explicit attributes.
    Construct(u16, &'static [Expr]),
    /// A value converted to a defined type, by index into `SchemaTables::defined_types`.
    Convert(u16, &'static Expr),
    /// `[a, b : repetition, ...]`.
    Aggregate(&'static [Element]),
    /// `{low op item op high}`; the flags are `true` for `<=`.
    Interval {
        /// The lower bound.
        low: &'static Expr,
        /// Whether the lower bound is included.
        low_inclusive: bool,
        /// The tested value.
        item: &'static Expr,
        /// Whether the upper bound is included.
        high_inclusive: bool,
        /// The upper bound.
        high: &'static Expr,
    },
    /// `QUERY(variable <* aggregate | condition)`.
    Query {
        /// The slot of the variable.
        variable: u16,
        /// The aggregate queried.
        aggregate: &'static Expr,
        /// The condition each element is kept for.
        condition: &'static Expr,
    },
}

/// An element of an aggregate initializer.
#[derive(Debug, PartialEq)]
pub struct Element {
    /// The value.
    pub value: Expr,
    /// The repetition count (`value : count`).
    pub repeat: Option<&'static Expr>,
}

/// A statement.
#[derive(Debug, PartialEq)]
pub enum Stmt {
    /// `;`.
    Null,
    /// `target := value`; the target is a variable followed by attribute and index qualifiers.
    Assign(&'static Expr, &'static Expr),
    /// `IF`.
    If {
        /// The condition.
        condition: &'static Expr,
        /// Executed when the condition is `TRUE`.
        then: &'static [Stmt],
        /// Executed when it is `FALSE` or `UNKNOWN`.
        otherwise: &'static [Stmt],
    },
    /// `CASE`.
    Case {
        /// The selector.
        selector: &'static Expr,
        /// The branches in order.
        branches: &'static [Branch],
        /// The `OTHERWISE` branch.
        otherwise: Option<&'static Stmt>,
    },
    /// `REPEAT`.
    Repeat {
        /// `variable := from TO to [BY by]`.
        increment: Option<&'static Increment>,
        /// `WHILE`.
        while_: Option<&'static Expr>,
        /// `UNTIL`.
        until: Option<&'static Expr>,
        /// The body.
        body: &'static [Stmt],
    },
    /// `RETURN`.
    Return(Option<&'static Expr>),
    /// `ESCAPE`.
    Escape,
    /// `SKIP`.
    Skip,
    /// `BEGIN ... END`.
    Compound(&'static [Stmt]),
}

/// A branch of a `CASE` statement.
#[derive(Debug, PartialEq)]
pub struct Branch {
    /// The labels compared with the selector.
    pub labels: &'static [Expr],
    /// The statement.
    pub body: Stmt,
}

/// The increment control of a `REPEAT` statement.
#[derive(Debug, PartialEq)]
pub struct Increment {
    /// The slot of the loop variable.
    pub variable: u16,
    /// The first value.
    pub from: Expr,
    /// The last value.
    pub to: Expr,
    /// The step, 1 when absent.
    pub by: Option<Expr>,
}

/// A local variable or constant of a function or global rule.
#[derive(Debug, PartialEq)]
pub struct Local {
    /// The slot.
    pub slot: u16,
    /// The initial value; `?` when absent.
    pub init: Option<Expr>,
    /// For an aggregate variable, its kind: an aggregate assigned to the variable takes it.
    pub kind: Option<super::AggregateKind>,
    /// For an `ARRAY`, the lower index bound: an aggregate assigned to the variable takes it.
    pub lower: Option<Expr>,
}

/// The body of a function or global rule.
#[derive(Debug, PartialEq)]
pub struct Algorithm {
    /// The number of parameters; they occupy the first slots.
    pub parameters: u16,
    /// The local variables and constants in declared order.
    pub locals: &'static [Local],
    /// The statements.
    pub body: &'static [Stmt],
}
