//! Der Syntaxbaum, in ESTree-Form.
//!
//! ESTree, weil es die Form ist, die jedes Werkzeug der Welt spricht — acorn,
//! esprima, babel. Das ist keine Bequemlichkeit: es macht den Baum gegen einen
//! zweiten Parser vergleichbar, und ein Orakel, das man nicht selbst geschrieben
//! hat, ist das einzige, das einen eigenen Fehler findet.
//!
//! Kein Arena, keine Ids: `Box` und `Vec`. Der Baum wird einmal gebaut und dann
//! gelesen; eine Arena spart Allokationen, die hier niemand zaehlt, und kostet
//! Lesbarkeit an jeder Stelle.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub body: Vec<Stmt>,
    /// `module` heisst: `import`/`export` erlaubt, immer streng, `await` oben
    /// erlaubt. Das ist kein Schalter am Parser, es ist eine andere Grammatik.
    pub module: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    Block(Vec<Stmt>),
    Empty,
    Debugger,
    Return(Option<Expr>),
    If { test: Expr, cons: Box<Stmt>, alt: Option<Box<Stmt>> },
    For { init: Option<Box<ForInit>>, test: Option<Expr>, update: Option<Expr>, body: Box<Stmt> },
    ForIn { left: Box<ForHead>, right: Expr, body: Box<Stmt> },
    ForOf { left: Box<ForHead>, right: Expr, body: Box<Stmt>, is_await: bool },
    While { test: Expr, body: Box<Stmt> },
    DoWhile { body: Box<Stmt>, test: Expr },
    Break(Option<String>),
    Continue(Option<String>),
    Labeled { label: String, body: Box<Stmt> },
    Switch { disc: Expr, cases: Vec<SwitchCase> },
    Throw(Expr),
    Try { block: Vec<Stmt>, handler: Option<CatchClause>, finalizer: Option<Vec<Stmt>> },
    With { obj: Expr, body: Box<Stmt> },
    VarDecl(VarDecl),
    Func(Rc<Func>),
    Class(Rc<Class>),
    Import(Import),
    ExportNamed { decl: Option<Box<Stmt>>, specifiers: Vec<ExportSpec>, source: Option<String> },
    ExportDefault(Box<ExportDefault>),
    ExportAll { source: String, alias: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExportDefault { Expr(Expr), Func(Rc<Func>), Class(Rc<Class>) }

#[derive(Debug, Clone, PartialEq)]
pub enum ForInit { VarDecl(VarDecl), Expr(Expr) }

#[derive(Debug, Clone, PartialEq)]
pub enum ForHead { VarDecl(VarDecl), Pattern(Pat) }

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase { pub test: Option<Expr>, pub body: Vec<Stmt> }

#[derive(Debug, Clone, PartialEq)]
pub struct CatchClause { pub param: Option<Pat>, pub body: Vec<Stmt> }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind { Var, Let, Const }

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl { pub kind: VarKind, pub decls: Vec<Declarator> }

#[derive(Debug, Clone, PartialEq)]
pub struct Declarator { pub id: Pat, pub init: Option<Expr> }

/// Ein Bindungsmuster. Dass Muster und Ausdruecke sich ueberlappen (`[a, b]`
/// ist beides, bis das `=` kommt) ist die zentrale Schwierigkeit der Grammatik;
/// der Parser liest deshalb erst einen Ausdruck und biegt ihn um
/// (`expr_to_pattern`), statt vorauszuschauen.
#[derive(Debug, Clone, PartialEq)]
pub enum Pat {
    Ident(String),
    Array(Vec<Option<Pat>>),
    Object { props: Vec<ObjPatProp>, rest: Option<Box<Pat>> },
    Assign { left: Box<Pat>, right: Box<Expr> },
    Rest(Box<Pat>),
    /// `[a.b] = c` — ein Ziel, das kein Bezeichner ist. In einer Deklaration
    /// verboten, in einer Zuweisung erlaubt.
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjPatProp { pub key: PropKey, pub value: Pat, pub computed: bool, pub shorthand: bool }

#[derive(Debug, Clone, PartialEq)]
pub enum PropKey { Ident(String), Str(String), Num(f64), Computed(Box<Expr>), Private(String) }

#[derive(Debug, Clone, PartialEq)]
pub struct Func {
    pub name: Option<String>,
    pub params: Vec<Pat>,
    pub body: Vec<Stmt>,
    pub is_async: bool,
    pub is_generator: bool,
    /// Ein Pfeil mit Ausdruckskoerper (`x => x*2`). Der Koerper steht dann als
    /// einzelnes `Stmt::Return` in `body`, damit alles darunter EINEN Fall hat.
    pub is_arrow: bool,
    pub expr_body: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Class {
    pub name: Option<String>,
    pub super_class: Option<Expr>,
    pub body: Vec<ClassMember>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassMember {
    Method { key: PropKey, func: Rc<Func>, kind: MethodKind, is_static: bool, computed: bool },
    Field { key: PropKey, value: Option<Expr>, is_static: bool, computed: bool },
    StaticBlock(Vec<Stmt>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind { Method, Get, Set, Constructor }

#[derive(Debug, Clone, PartialEq)]
pub struct Import { pub specifiers: Vec<ImportSpec>, pub source: String }

#[derive(Debug, Clone, PartialEq)]
pub enum ImportSpec {
    Default(String),
    Namespace(String),
    Named { imported: String, local: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportSpec { pub local: String, pub exported: String }

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Ident(String),
    Num(f64),
    BigInt(String),
    Str(String),
    Bool(bool),
    Null,
    This,
    Super,
    Regex { body: String, flags: String },
    Template { quasis: Vec<TemplateElement>, exprs: Vec<Expr> },
    TaggedTemplate { tag: Box<Expr>, quasis: Vec<TemplateElement>, exprs: Vec<Expr> },
    Array(Vec<Option<Expr>>),
    Object(Vec<ObjProp>),
    Func(Rc<Func>),
    Class(Rc<Class>),
    Unary { op: UnaryOp, arg: Box<Expr> },
    Update { op: UpdateOp, arg: Box<Expr>, prefix: bool },
    Binary { op: BinOp, left: Box<Expr>, right: Box<Expr> },
    Logical { op: LogicalOp, left: Box<Expr>, right: Box<Expr> },
    Assign { op: AssignOp, left: Box<Pat>, right: Box<Expr> },
    Cond { test: Box<Expr>, cons: Box<Expr>, alt: Box<Expr> },
    Call { callee: Box<Expr>, args: Vec<Arg>, optional: bool },
    New { callee: Box<Expr>, args: Vec<Arg> },
    Member { obj: Box<Expr>, prop: Box<MemberProp>, optional: bool },
    /// Die Kette um ein `?.`, damit ein Kurzschluss die GANZE Kette abbricht
    /// und nicht nur das eine Glied.
    Chain(Box<Expr>),
    Seq(Vec<Expr>),
    Spread(Box<Expr>),
    Yield { arg: Option<Box<Expr>>, delegate: bool },
    Await(Box<Expr>),
    /// `new.target` / `import.meta`
    MetaProp { meta: String, prop: String },
    ImportCall(Vec<Arg>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MemberProp { Ident(String), Private(String), Computed(Expr) }

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateElement { pub cooked: Option<String>, pub raw: String }

#[derive(Debug, Clone, PartialEq)]
pub enum Arg { Expr(Expr), Spread(Expr) }

#[derive(Debug, Clone, PartialEq)]
pub struct ObjProp {
    pub key: PropKey,
    pub value: ObjPropValue,
    pub computed: bool,
    pub shorthand: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjPropValue { Init(Expr), Get(Rc<Func>), Set(Rc<Func>), Method(Rc<Func>), Spread(Expr) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp { Minus, Plus, Bang, Tilde, Typeof, Void, Delete }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOp { Inc, Dec }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp { And, Or, Nullish }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod, Exp,
    Lt, Gt, LtEq, GtEq, EqEq, NotEq, EqEqEq, NotEqEq,
    Shl, Shr, UShr, BitAnd, BitOr, BitXor, In, Instanceof,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign, Add, Sub, Mul, Div, Mod, Exp, Shl, Shr, UShr,
    BitAnd, BitOr, BitXor, And, Or, Nullish,
}
