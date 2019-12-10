use std::ops::{Deref, DerefMut, Index, IndexMut};
use std::convert::TryInto;
use std::iter::FromIterator;
use std::sync::Arc;
use std::fmt::Write;
use std::hash::Hash;
use std::collections::HashMap;
use super::{ElabError, BoxError};
use crate::util::*;
use super::lisp::{LispVal, UNDEF, LispRemapper};
pub use crate::parser::ast::{Modifiers, Prec};
pub use crate::lined_string::{Span, FileSpan};

#[derive(Copy, Clone, Hash, PartialEq, Eq)] pub struct SortID(pub u8);
#[derive(Copy, Clone, Hash, PartialEq, Eq)] pub struct TermID(pub u32);
#[derive(Copy, Clone, Hash, PartialEq, Eq)] pub struct ThmID(pub u32);
#[derive(Copy, Clone, Hash, PartialEq, Eq)] pub struct AtomID(pub u32);

macro_rules! vec_index {
  ($vec:ident, $id:ty) => {
    #[derive(Clone)]
    pub struct $vec<T>(pub Vec<T>);

    impl<T> $vec<T> {
      pub fn get(&self, i: $id) -> Option<&T> { self.0.get(i.0 as usize) }
      pub fn get_mut(&mut self, i: $id) -> Option<&mut T> { self.0.get_mut(i.0 as usize) }
    }
    impl<T> Default for $vec<T> {
      fn default() -> $vec<T> { $vec(Vec::new()) }
    }
    impl<T> Index<$id> for $vec<T> {
      type Output = T;
      fn index(&self, i: $id) -> &T { &self.0[i.0 as usize] }
    }
    impl<T> IndexMut<$id> for $vec<T> {
      fn index_mut(&mut self, i: $id) -> &mut T { &mut self.0[i.0 as usize] }
    }
    impl<T> Deref for $vec<T> {
      type Target = Vec<T>;
      fn deref(&self) -> &Vec<T> { &self.0 }
    }
    impl<T> DerefMut for $vec<T> {
      fn deref_mut(&mut self) -> &mut Vec<T> { &mut self.0 }
    }
    impl<T> FromIterator<T> for $vec<T> {
      fn from_iter<I: IntoIterator<Item=T>>(iter: I) -> Self { $vec(Vec::from_iter(iter)) }
    }
  };
}

vec_index!(SortVec, SortID);
vec_index!(TermVec, TermID);
vec_index!(ThmVec, ThmID);
vec_index!(AtomVec, AtomID);

#[derive(Clone)]
pub struct Sort {
  pub name: ArcString,
  pub span: FileSpan,
  pub mods: Modifiers,
}

#[derive(Copy, Clone)]
pub enum Type {
  Bound(SortID),
  Reg(SortID, u64),
}

/// An `ExprNode` is interpreted inside a context containing the `Vec<(String, Type)>`
/// args and the `Vec<ExprNode>` heap.
///   * `Ref(n)` is variable n, if `n < args.len()`
///   * `Ref(n + args.len())` is a reference to heap element `n`
///   * `Dummy(s, sort)` is a fresh dummy variable `s` with sort `sort`
///   * `App(t, nodes)` is an application of term constructor `t` to subterms
#[derive(Clone)]
pub enum ExprNode {
  Ref(usize),
  Dummy(String, SortID),
  App(TermID, Vec<ExprNode>),
}

/// The Expr type stores expression dags using a local context of expression nodes
/// and a final expression. See `ExprNode` for explanation of the variants.
#[derive(Clone)]
pub struct Expr {
  pub heap: Vec<ExprNode>,
  pub head: ExprNode,
}

#[derive(Clone)]
pub struct Term {
  pub span: FileSpan,
  pub vis: Modifiers,
  pub id: Span,
  pub args: Vec<(String, Type)>,
  pub ret: Type,
  pub val: Option<Expr>,
}

#[derive(Clone)]
pub enum ProofNode {
  Ref(usize),
  Term { term: TermID, args: Vec<ProofNode> },
  Thm {
    thm: ThmID,
    args: Vec<ProofNode>,
    hyps: Vec<ProofNode>,
    tgt: Box<ProofNode>,
  },
  Conv { tgt: Box<ProofNode>, proof: Box<ProofNode> },
}

/// The Proof type stores Proofession dags using a local context of Proofession nodes
/// and a final Proofession. See `ProofNode` for explanation of the variants.
#[derive(Clone)]
pub struct Proof {
  pub heap: Vec<ProofNode>,
  pub head: ProofNode,
}

#[derive(Clone)]
pub struct Thm {
  pub span: FileSpan,
  pub vis: Modifiers,
  pub id: Span,
  pub args: Vec<(String, Type)>,
  pub heap: Vec<ExprNode>,
  pub hyps: Vec<ExprNode>,
  pub ret: ExprNode,
  pub proof: Option<Proof>,
}

#[derive(Clone)]
pub enum StmtTrace {
  Sort(ArcString),
  Decl(ArcString),
}

#[derive(Copy, Clone)]
pub enum DeclKey {
  Term(TermID),
  Thm(ThmID),
}

#[derive(Clone)]
pub enum Literal {
  Var(usize, Prec),
  Const(ArcString),
}

#[derive(Clone)]
pub struct NotaInfo {
  pub span: FileSpan,
  pub term: TermID,
  pub nargs: usize,
  pub rassoc: Option<bool>,
  pub lits: Vec<Literal>,
}

#[derive(Clone)]
pub enum Coe {
  One(FileSpan, TermID),
  Trans(Arc<Coe>, SortID, Arc<Coe>),
}

impl Coe {
  fn write_arrows_r(&self, sorts: &SortVec<Sort>, s: &mut String, related: &mut Vec<(FileSpan, BoxError)>,
      sl: SortID, sr: SortID) -> Result<(), std::fmt::Error> {
    match self {
      Coe::One(fsp, _) => {
        related.push((fsp.clone(), format!("{} -> {}", sorts[sl].name, sorts[sr].name).into()));
        write!(s, " -> {}", sorts[sr].name)
      }
      &Coe::Trans(ref c1, sm, ref c2) => {
        c1.write_arrows_r(sorts, s, related, sl, sm)?;
        c2.write_arrows_r(sorts, s, related, sm, sr)
      }
    }
  }

  fn write_arrows(&self, sorts: &SortVec<Sort>, s: &mut String, related: &mut Vec<(FileSpan, BoxError)>,
      s1: SortID, s2: SortID) -> Result<(), std::fmt::Error> {
    write!(s, "{}", sorts[s1].name)?;
    self.write_arrows_r(sorts, s, related, s1, s2)
  }
}

#[derive(Default, Clone)]
pub struct ParserEnv {
  pub delims_l: Delims,
  pub delims_r: Delims,
  pub consts: HashMap<ArcString, (FileSpan, Prec)>,
  pub prec_assoc: HashMap<u32, (FileSpan, bool)>,
  pub prefixes: HashMap<ArcString, NotaInfo>,
  pub infixes: HashMap<ArcString, NotaInfo>,
  pub coes: HashMap<SortID, HashMap<SortID, Arc<Coe>>>,
  pub coe_prov: HashMap<SortID, SortID>,
}

#[derive(Default, Clone)]
pub struct Environment {
  pub sort_keys: HashMap<ArcString, SortID>,
  pub sorts: SortVec<Sort>,
  pub pe: ParserEnv,
  pub decl_keys: HashMap<ArcString, DeclKey>,
  pub terms: TermVec<Term>,
  pub thms: ThmVec<Thm>,
  pub atoms: HashMap<ArcString, AtomID>,
  pub lisp_ctx: AtomVec<(ArcString, LispVal)>,
  pub stmts: Vec<StmtTrace>,
}

#[derive(Default, Clone)]
pub struct Delims([u8; 32]);

impl Delims {
  pub fn get(&self, c: u8) -> bool { self.0[(c >> 3) as usize] & (1 << (c & 7)) != 0 }
  pub fn set(&mut self, c: u8) { self.0[(c >> 3) as usize] |= 1 << (c & 7) }
  pub fn merge(&mut self, other: &Self) {
    for i in 0..32 { self.0[i] |= other.0[i] }
  }
}

#[derive(Default)]
struct Remapper {
  sort: HashMap<SortID, SortID>,
  term: HashMap<TermID, TermID>,
  thm: HashMap<ThmID, ThmID>,
}

pub trait Remap<R> {
  fn remap(&self, r: &mut R) -> Self;
}
impl Remap<Remapper> for SortID {
  fn remap(&self, r: &mut Remapper) -> Self { *r.sort.get(self).unwrap_or(self) }
}
impl Remap<Remapper> for TermID {
  fn remap(&self, r: &mut Remapper) -> Self { *r.term.get(self).unwrap_or(self) }
}
impl Remap<Remapper> for ThmID {
  fn remap(&self, r: &mut Remapper) -> Self { *r.thm.get(self).unwrap_or(self) }
}
impl<R> Remap<R> for String {
  fn remap(&self, _: &mut R) -> Self { self.clone() }
}
impl<R, A: Remap<R>, B: Remap<R>> Remap<R> for (A, B) {
  fn remap(&self, r: &mut R) -> Self { (self.0.remap(r), self.1.remap(r)) }
}
impl<R, A: Remap<R>> Remap<R> for Option<A> {
  fn remap(&self, r: &mut R) -> Self { self.as_ref().map(|x| x.remap(r)) }
}
impl<R, A: Remap<R>> Remap<R> for Vec<A> {
  fn remap(&self, r: &mut R) -> Self { self.iter().map(|x| x.remap(r)).collect() }
}
impl<R, A: Remap<R>> Remap<R> for Box<A> {
  fn remap(&self, r: &mut R) -> Self { Box::new(self.deref().remap(r)) }
}
impl<R, A: Remap<R>> Remap<R> for Arc<A> {
  fn remap(&self, r: &mut R) -> Self { Arc::new(self.deref().remap(r)) }
}
impl Remap<Remapper> for Type {
  fn remap(&self, r: &mut Remapper) -> Self {
    match self {
      Type::Bound(s) => Type::Bound(s.remap(r)),
      &Type::Reg(s, deps) => Type::Reg(s.remap(r), deps),
    }
  }
}
impl Remap<Remapper> for ExprNode {
  fn remap(&self, r: &mut Remapper) -> Self {
    match self {
      &ExprNode::Ref(i) => ExprNode::Ref(i),
      ExprNode::Dummy(i, s) => ExprNode::Dummy(i.clone(), s.remap(r)),
      ExprNode::App(t, es) => ExprNode::App(t.remap(r), es.remap(r)),
    }
  }
}
impl Remap<Remapper> for Expr {
  fn remap(&self, r: &mut Remapper) -> Self {
    Expr {
      heap: self.heap.remap(r),
      head: self.head.remap(r),
    }
  }
}
impl Remap<Remapper> for Term {
  fn remap(&self, r: &mut Remapper) -> Self {
    Term {
      span: self.span.clone(),
      vis: self.vis,
      id: self.id,
      args: self.args.remap(r),
      ret: self.ret.remap(r),
      val: self.val.remap(r),
    }
  }
}
impl Remap<Remapper> for ProofNode {
  fn remap(&self, r: &mut Remapper) -> Self {
    match self {
      &ProofNode::Ref(i) => ProofNode::Ref(i),
      ProofNode::Term {term, args} => ProofNode::Term { term: term.remap(r), args: args.remap(r) },
      ProofNode::Thm {thm, args, hyps, tgt} => ProofNode::Thm {
        thm: thm.remap(r), args: args.remap(r), hyps: hyps.remap(r), tgt: tgt.remap(r) },
      ProofNode::Conv {tgt, proof} => ProofNode::Conv { tgt: tgt.remap(r), proof: proof.remap(r) },
    }
  }
}
impl Remap<Remapper> for Proof {
  fn remap(&self, r: &mut Remapper) -> Self {
    Proof {
      heap: self.heap.remap(r),
      head: self.head.remap(r),
    }
  }
}
impl Remap<Remapper> for Thm {
  fn remap(&self, r: &mut Remapper) -> Self {
    Thm {
      span: self.span.clone(),
      vis: self.vis,
      id: self.id,
      args: self.args.remap(r),
      heap: self.heap.remap(r),
      hyps: self.hyps.remap(r),
      ret: self.ret.remap(r),
      proof: self.proof.remap(r),
    }
  }
}
impl Remap<Remapper> for NotaInfo {
  fn remap(&self, r: &mut Remapper) -> Self {
    NotaInfo {
      span: self.span.clone(),
      term: self.term.remap(r),
      nargs: self.nargs,
      rassoc: self.rassoc,
      lits: self.lits.clone(),
    }
  }
}
impl Remap<Remapper> for Coe {
  fn remap(&self, r: &mut Remapper) -> Self {
    match self {
      Coe::One(sp, t) => Coe::One(sp.clone(), t.remap(r)),
      Coe::Trans(c1, s, c2) => Coe::Trans(c1.remap(r), s.remap(r), c2.remap(r)),
    }
  }
}

pub struct IncompatibleError {
  pub decl1: FileSpan,
  pub decl2: FileSpan,
}

impl ParserEnv {
  pub fn add_delimiters(&mut self, ls: &[u8], rs: &[u8]) {
    for &c in ls { self.delims_l.set(c) }
    for &c in rs { self.delims_r.set(c) }
  }

  pub fn add_const(&mut self, tk: ArcString, sp: FileSpan, p: Prec) -> Result<(), IncompatibleError> {
    if let Some((_, e)) = self.consts.try_insert(tk, (sp.clone(), p)) {
      if e.get().1 == p { return Ok(()) }
      Err(IncompatibleError { decl1: e.get().0.clone(), decl2: sp })
    } else { Ok(()) }
  }

  pub fn add_prec_assoc(&mut self, p: u32, sp: FileSpan, r: bool) -> Result<(), IncompatibleError> {
    if let Some((_, e)) = self.prec_assoc.try_insert(p, (sp.clone(), r)) {
      if e.get().1 == r { return Ok(()) }
      let (decl1, decl2) = if r { (e.get().0.clone(), sp) } else { (sp, e.get().0.clone()) };
      Err(IncompatibleError {decl1, decl2})
    } else { Ok(()) }
  }

  fn add_nota_info(m: &mut HashMap<ArcString, NotaInfo>, tk: ArcString, n: NotaInfo) -> Result<(), IncompatibleError> {
    if let Some((n, e)) = m.try_insert(tk.clone(), n) {
      if e.get().span == n.span { return Ok(()) }
      Err(IncompatibleError { decl1: e.get().span.clone(), decl2: n.span })
    } else { Ok(()) }
  }

  pub fn add_prefix(&mut self, tk: ArcString, n: NotaInfo) -> Result<(), IncompatibleError> {
    Self::add_nota_info(&mut self.prefixes, tk, n)
  }
  pub fn add_infix(&mut self, tk: ArcString, n: NotaInfo) -> Result<(), IncompatibleError> {
    Self::add_nota_info(&mut self.infixes, tk, n)
  }

  fn add_coe1(&mut self, sp: Span, sorts: &SortVec<Sort>, s1: SortID, s2: SortID, c: Arc<Coe>) -> Result<(), ElabError> {
    if s1 == s2 {
      let mut err = "coercion cycle detected: ".to_owned();
      let mut related = Vec::new();
      c.write_arrows(sorts, &mut err, &mut related, s1, s2).unwrap();
      return Err(ElabError::with_info(sp, err.into(), related))
    }
    if let Some((c, e)) = self.coes.entry(s1).or_default().try_insert(s2, c) {
      let mut err = "coercion diamond detected: ".to_owned();
      let mut related = Vec::new();
      e.get().write_arrows(sorts, &mut err, &mut related, s1, s2).unwrap();
      err.push_str(";   ");
      c.write_arrows(sorts, &mut err, &mut related, s1, s2).unwrap();
      return Err(ElabError::with_info(sp, err.into(), related))
    }
    Ok(())
  }

  fn update_provs(&mut self, sp: Span, sorts: &SortVec<Sort>) -> Result<(), ElabError> {
    let mut provs = HashMap::new();
    for (&s1, m) in &self.coes {
      for (&s2, _) in m {
        if sorts[s2].mods.contains(Modifiers::PROVABLE) {
          if let Some(s2_) = provs.insert(s1, s2) {
            let mut err = "coercion diamond to provable detected:\n".to_owned();
            let mut related = Vec::new();
            self.coes[&s1][&s2].write_arrows(sorts, &mut err, &mut related, s1, s2).unwrap();
            err.push_str(" provable\n");
            self.coes[&s1][&s2_].write_arrows(sorts, &mut err, &mut related, s1, s2_).unwrap();
            err.push_str(" provable");
            return Err(ElabError::with_info(sp, err.into(), related))
          }
        }
      }
    }
    self.coe_prov = provs;
    Ok(())
  }

  fn add_coe_raw(&mut self, sp: Span, sorts: &SortVec<Sort>,
      s1: SortID, s2: SortID, fsp: FileSpan, t: TermID) -> Result<(), ElabError> {
    let c1 = Arc::new(Coe::One(fsp, t));
    let mut todo = Vec::new();
    for (&sl, m) in &self.coes {
      if let Some(c) = m.get(&s1) {
        todo.push((sl, s2, Arc::new(Coe::Trans(c.clone(), s1, c1.clone()))));
      }
    }
    todo.push((s1, s2, c1.clone()));
    if let Some(m) = self.coes.get(&s2) {
      for (&sr, c) in m {
        todo.push((s1, sr, Arc::new(Coe::Trans(c1.clone(), s2, c.clone()))));
      }
    }
    for (sl, sr, c) in todo { self.add_coe1(sp, sorts, sl, sr, c)? }
    Ok(())
  }

  pub fn add_coe(&mut self, sp: Span, sorts: &SortVec<Sort>,
      s1: SortID, s2: SortID, fsp: FileSpan, t: TermID) -> Result<(), ElabError> {
    self.add_coe_raw(sp, sorts, s1, s2, fsp, t)?;
    self.update_provs(sp, sorts)
  }

  fn merge(&mut self, other: &Self, r: &mut Remapper, sp: Span, sorts: &SortVec<Sort>, errors: &mut Vec<ElabError>) {
    self.delims_l.merge(&other.delims_l);
    self.delims_r.merge(&other.delims_r);
    for (tk, &(ref fsp, p)) in &other.consts {
      self.add_const(tk.clone(), fsp.clone(), p).unwrap_or_else(|r|
        errors.push(ElabError::with_info(sp,
          format!("constant '{}' declared with two precedences", tk).into(),
          vec![(r.decl1, "declared here".into()), (r.decl2, "declared here".into())])))
    }
    for (&p, &(ref fsp, r)) in &other.prec_assoc {
      self.add_prec_assoc(p, fsp.clone(), r).unwrap_or_else(|r|
        errors.push(ElabError::with_info(sp,
          format!("precedence level {} has incompatible associativity", p).into(),
          vec![(r.decl1, "left assoc here".into()), (r.decl2, "right assoc here".into())])))
    }
    for (tk, i) in &other.prefixes {
      self.add_prefix(tk.clone(), i.remap(r)).unwrap_or_else(|r|
        errors.push(ElabError::with_info(sp,
          format!("constant '{}' declared twice", tk).into(),
          vec![(r.decl1, "declared here".into()), (r.decl2, "declared here".into())])))
    }
    for (tk, i) in &other.infixes {
      self.add_infix(tk.clone(), i.remap(r)).unwrap_or_else(|r|
        errors.push(ElabError::with_info(sp,
          format!("constant '{}' declared twice", tk).into(),
          vec![(r.decl1, "declared here".into()), (r.decl2, "declared here".into())])))
    }
    for (&s1, m) in &other.coes {
      for (&s2, coe) in m {
        if let &Coe::One(ref fsp, t) = coe.deref() {
          self.add_coe_raw(sp, sorts, s1, s2, fsp.clone(), t.remap(r))
            .unwrap_or_else(|r| errors.push(r))
        }
      }
    }
    self.update_provs(sp, sorts).unwrap_or_else(|r| errors.push(r))
  }
}

pub struct RedeclarationError {
  pub msg: String,
  pub othermsg: String,
  pub other: FileSpan
}

impl Environment {
  pub fn term(&self, s: &str) -> Option<TermID> {
    if let DeclKey::Term(i) = self.decl_keys[s] { Some(i) } else { None }
  }

  pub fn thm(&self, s: &str) -> Option<ThmID> {
    if let DeclKey::Thm(i) = self.decl_keys[s] { Some(i) } else { None }
  }
}

pub enum AddItemError<A> {
  Redeclaration(A, RedeclarationError),
  Overflow
}
type AddItemResult<A> = Result<A, AddItemError<Option<A>>>;

impl Environment {
  pub fn add_sort(&mut self, s: ArcString, fsp: FileSpan, sd: Modifiers) -> Result<SortID, AddItemError<SortID>> {
    let new_id = SortID(self.sorts.len().try_into().map_err(|_| AddItemError::Overflow)?);
    if let Some((_, e)) = self.sort_keys.try_insert(s.clone(), new_id) {
      let old_id = *e.get();
      let ref sort = self.sorts[old_id];
      if sd == sort.mods { Ok(old_id) }
      else {
        Err(AddItemError::Redeclaration(old_id, RedeclarationError {
          msg: format!("sort '{}' redeclared", e.key()),
          othermsg: "previously declared here".to_owned(),
          other: sort.span.clone()
        }))
      }
    } else {
      self.sorts.push(Sort { name: s.clone(), span: fsp, mods: sd });
      self.stmts.push(StmtTrace::Sort(s));
      Ok(new_id)
    }
  }

  pub fn add_term(&mut self, s: ArcString, new: FileSpan, t: impl FnOnce() -> Term) -> AddItemResult<TermID> {
    let new_id = TermID(self.terms.len().try_into().map_err(|_| AddItemError::Overflow)?);
    if let Some((_, e)) = self.decl_keys.try_insert(s.clone(), DeclKey::Term(new_id)) {
      let (res, sp) = match *e.get() {
        DeclKey::Term(old_id) => {
          let ref sp = self.terms[old_id].span;
          if *sp == new { return Ok(old_id) }
          (Some(old_id), sp)
        }
        DeclKey::Thm(old_id) => (None, &self.thms[old_id].span)
      };
      Err(AddItemError::Redeclaration(res, RedeclarationError {
        msg: format!("term '{}' redeclared", e.key()),
        othermsg: "previously declared here".to_owned(),
        other: sp.clone()
      }))
    } else {
      self.terms.push(t());
      self.stmts.push(StmtTrace::Decl(s));
      Ok(new_id)
    }
  }

  pub fn add_thm(&mut self, s: ArcString, new: FileSpan, t: impl FnOnce() -> Thm) -> AddItemResult<ThmID> {
    let new_id = ThmID(self.thms.len().try_into().map_err(|_| AddItemError::Overflow)?);
    if let Some((_, e)) = self.decl_keys.try_insert(s.clone(), DeclKey::Thm(new_id)) {
      let (res, sp) = match *e.get() {
        DeclKey::Thm(old_id) => {
          let ref sp = self.thms[old_id].span;
          if *sp == new { return Ok(old_id) }
          (Some(old_id), sp)
        }
        DeclKey::Term(old_id) => (None, &self.terms[old_id].span)
      };
      Err(AddItemError::Redeclaration(res, RedeclarationError {
        msg: format!("theorem '{}' redeclared", e.key()),
        othermsg: "previously declared here".to_owned(),
        other: sp.clone()
      }))
    } else {
      self.thms.push(t());
      self.stmts.push(StmtTrace::Decl(s));
      Ok(new_id)
    }
  }

  pub fn add_coe(&mut self, s1: SortID, s2: SortID, fsp: FileSpan, t: TermID) -> Result<(), ElabError> {
    self.pe.add_coe(fsp.span, &self.sorts, s1, s2, fsp, t)
  }

  pub fn get_atom(&mut self, s: ArcString) -> AtomID {
    let ctx = &mut self.lisp_ctx;
    *self.atoms.entry(s.clone()).or_insert_with(move ||
      (AtomID(ctx.len().try_into().expect("too many atoms")), ctx.push((s, UNDEF.clone()))).0)
  }

  pub fn merge(&mut self, other: &Self, sp: Span, errors: &mut Vec<ElabError>) -> Result<(), ElabError> {
    if self.stmts.is_empty() { return Ok(*self = other.clone()) }
    let mut remap = Remapper::default();
    for s in &other.stmts {
      match s {
        StmtTrace::Sort(s) => {
          let i = other.sort_keys[s];
          let ref sort = other.sorts[i];
          let id = match self.add_sort(s.clone(), sort.span.clone(), sort.mods) {
            Ok(id) => id,
            Err(AddItemError::Redeclaration(id, r)) => {
              errors.push(ElabError::with_info(sp, r.msg.into(), vec![
                (sort.span.clone(), r.othermsg.clone().into()),
                (r.other, r.othermsg.into())
              ]));
              id
            }
            Err(AddItemError::Overflow) => Err(ElabError::new_e(sp, "too many sorts"))?
          };
          if i != id { remap.sort.insert(i, id); }
        }
        StmtTrace::Decl(s) => match other.decl_keys[s] {
          DeclKey::Term(i) => {
            let ref o = other.terms[i];
            let id = match self.add_term(s.clone(), o.span.clone(), || o.remap(&mut remap)) {
              Ok(id) => id,
              Err(AddItemError::Redeclaration(id, r)) => {
                let e = ElabError::with_info(sp, r.msg.into(), vec![
                  (o.span.clone(), r.othermsg.clone().into()),
                  (r.other, r.othermsg.into())
                ]);
                match id { None => Err(e)?, Some(id) => {errors.push(e); id} }
              }
              Err(AddItemError::Overflow) => Err(ElabError::new_e(sp, "too many terms"))?
            };
            if i != id { remap.term.insert(i, id); }
          }
          DeclKey::Thm(i) => {
            let ref o = other.thms[i];
            let id = match self.add_thm(s.clone(), o.span.clone(), || o.remap(&mut remap)) {
              Ok(id) => id,
              Err(AddItemError::Redeclaration(id, r)) => {
                let e = ElabError::with_info(sp, r.msg.into(), vec![
                  (o.span.clone(), r.othermsg.clone().into()),
                  (r.other, r.othermsg.into())
                ]);
                match id { None => Err(e)?, Some(id) => {errors.push(e); id} }
              }
              Err(AddItemError::Overflow) => Err(ElabError::new_e(sp, "too many theorems"))?
            };
            if i != id { remap.thm.insert(i, id); }
          }
        }
      }
    }
    self.pe.merge(&other.pe, &mut remap, sp, &self.sorts, errors);
    let mut r = LispRemapper {
      atom: other.lisp_ctx.iter().map(|(s, _)| self.get_atom(s.clone())).collect(),
      lisp: Default::default(),
    };
    for (i, (_, v)) in other.lisp_ctx.iter().enumerate() {
      self.lisp_ctx[r.atom[AtomID(i as u32)]].1 = v.remap(&mut r)
    }
    Ok(())
  }

  pub fn check_term_nargs(&self, sp: Span, term: TermID, nargs: usize) -> Result<(), ElabError> {
    let ref t = self.terms[term];
    if t.args.len() == nargs { return Ok(()) }
    Err(ElabError::with_info(sp, "incorrect number of arguments".into(),
      vec![(t.span.clone(), "declared here".into())]))
  }
}