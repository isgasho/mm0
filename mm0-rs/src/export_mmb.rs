use std::convert::TryInto;
use std::io::{self, Write, Seek};
use byteorder::{LE, ByteOrder, WriteBytesExt};
use crate::elab::environment::{
  Environment, Type, Expr, Proof, SortID, TermID, ThmID,
  TermVec, ThmVec, ExprNode, ProofNode, StmtTrace, DeclKey, Modifiers};

enum Value {
  U32(u32),
  U64(u64),
  Box(Box<[u8]>),
}

const DATA_8: u8  = 0x40;
const DATA_16: u8 = 0x80;
const DATA_32: u8 = 0xC0;

const STMT_SORT: u8  = 0x04;
const STMT_AXIOM: u8 = 0x02;
const STMT_TERM: u8  = 0x05;
const STMT_DEF: u8   = 0x05;
const STMT_THM: u8   = 0x06;
const STMT_LOCAL: u8 = 0x08;

const PROOF_TERM: u8      = 0x10;
const PROOF_TERM_SAVE: u8 = 0x11;
const PROOF_REF: u8       = 0x12;
const PROOF_DUMMY: u8     = 0x13;
const PROOF_THM: u8       = 0x14;
const PROOF_THM_SAVE: u8  = 0x15;
const PROOF_HYP: u8       = 0x16;
const PROOF_CONV: u8      = 0x17;
const PROOF_REFL: u8      = 0x18;
const PROOF_SYMM: u8      = 0x19;
const PROOF_CONG: u8      = 0x1A;
const PROOF_UNFOLD: u8    = 0x1B;
const PROOF_CONV_CUT: u8  = 0x1C;
const PROOF_CONV_REF: u8  = 0x1D;
const PROOF_CONV_SAVE: u8 = 0x1E;

const UNIFY_TERM: u8      = 0x30;
const UNIFY_TERM_SAVE: u8 = 0x31;
const UNIFY_REF: u8       = 0x32;
const UNIFY_DUMMY: u8     = 0x33;
const UNIFY_HYP: u8       = 0x36;

enum ProofCmd {
  Term(TermID),
  TermSave(TermID),
  Ref(u32),
  Dummy(SortID),
  Thm(ThmID),
  ThmSave(ThmID),
  Hyp,
  Conv,
  Refl,
  Sym,
  Cong,
  Unfold,
  ConvCut,
  ConvRef(u32),
  ConvSave,
}

enum UnifyCmd {
  Term(TermID),
  TermSave(TermID),
  Ref(u32),
  Dummy(SortID),
  Hyp,
}

struct Reorder {
  map: Box<[Option<u32>]>,
  idx: u32,
}

impl Reorder {
  fn new(nargs: u32, len: usize) -> Reorder {
    let mut map: Box<[Option<u32>]> = vec![None; len].into();
    for i in 0..nargs {map[i as usize] = Some(i)}
    Reorder {map, idx: nargs}
  }
}

pub struct Exporter<'a, W: Write + Seek + ?Sized> {
  env: &'a Environment,
  w: &'a mut W,
  pos: u64,
  term_reord: TermVec<Option<Reorder>>,
  thm_reord: ThmVec<Reorder>,
  fixups: Vec<(u64, Value)>,
}

#[must_use] struct Fixup32(u64);
#[must_use] struct Fixup64(u64);
#[must_use] struct FixupLarge(u64, Box<[u8]>);

impl Fixup32 {
  fn commit_val<'a, W: Write + Seek + ?Sized>(self, e: &mut Exporter<'a, W>, val: u32) {
    e.fixups.push((self.0, Value::U32(val)))
  }
  fn commit<'a, W: Write + Seek + ?Sized>(self, e: &mut Exporter<'a, W>) {
    let val = e.pos.try_into().unwrap();
    self.commit_val(e, val)
  }
}

impl Fixup64 {
  fn commit_val<'a, W: Write + Seek + ?Sized>(self, e: &mut Exporter<'a, W>, val: u64) {
    e.fixups.push((self.0, Value::U64(val)))
  }
  fn commit<'a, W: Write + Seek + ?Sized>(self, e: &mut Exporter<'a, W>) {
    let val = e.pos;
    self.commit_val(e, val)
  }
}

impl FixupLarge {
  fn commit<'a, W: Write + Seek + ?Sized>(self, e: &mut Exporter<'a, W>) {
    e.fixups.push((self.0, Value::Box(self.1)))
  }
}

impl<'a, W: Write + Seek + ?Sized> Write for Exporter<'a, W> {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    self.write_all(buf)?;
    Ok(buf.len())
  }
  fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
    self.pos += buf.len() as u64;
    self.w.write_all(buf)?;
    Ok(())
  }
  fn flush(&mut self) -> io::Result<()> {self.w.flush()}
}

fn write_cmd(w: &mut impl Write, cmd: u8, data: u32) -> io::Result<()> {
  if data == 0 {w.write_u8(cmd)}
  else if let Ok(data) = data.try_into() {
    w.write_u8(cmd | DATA_8)?;
    w.write_u8(data)
  } else if let Ok(data) = data.try_into() {
    w.write_u8(cmd | DATA_16)?;
    w.write_u16::<LE>(data)
  } else {
    w.write_u8(cmd | DATA_32)?;
    w.write_u32::<LE>(data)
  }
}

fn write_cmd_bytes(w: &mut impl Write, cmd: u8, vec: &[u8]) -> io::Result<()> {
  if let Ok(data) = (vec.len() + 2).try_into() {
    w.write_u8(cmd | DATA_8)?;
    w.write_u8(data)?;
    w.write_all(vec)
  } else if let Ok(data) = (vec.len() + 3).try_into() {
    w.write_u8(cmd | DATA_16)?;
    w.write_u16::<LE>(data)?;
    w.write_all(vec)
  } else {
    w.write_u8(cmd | DATA_32)?;
    w.write_u32::<LE>((vec.len() + 5).try_into().unwrap())?;
    w.write_all(vec)
  }
}

impl UnifyCmd {
  fn write_to(self, w: &mut impl Write) -> io::Result<()> {
    match self {
      UnifyCmd::Term(tid)     => write_cmd(w, UNIFY_TERM, tid.0),
      UnifyCmd::TermSave(tid) => write_cmd(w, UNIFY_TERM_SAVE, tid.0),
      UnifyCmd::Ref(n)        => write_cmd(w, UNIFY_REF, n),
      UnifyCmd::Dummy(sid)    => write_cmd(w, UNIFY_DUMMY, sid.0 as u32),
      UnifyCmd::Hyp           => w.write_u8(UNIFY_HYP),
    }
  }
}

impl ProofCmd {
  fn write_to(self, w: &mut impl Write) -> io::Result<()> {
    match self {
      ProofCmd::Term(tid)     => write_cmd(w, PROOF_TERM, tid.0),
      ProofCmd::TermSave(tid) => write_cmd(w, PROOF_TERM_SAVE, tid.0),
      ProofCmd::Ref(n)        => write_cmd(w, PROOF_REF, n),
      ProofCmd::Dummy(sid)    => write_cmd(w, PROOF_DUMMY, sid.0 as u32),
      ProofCmd::Thm(tid)      => write_cmd(w, PROOF_THM, tid.0),
      ProofCmd::ThmSave(tid)  => write_cmd(w, PROOF_THM_SAVE, tid.0),
      ProofCmd::Hyp           => w.write_u8(PROOF_HYP),
      ProofCmd::Conv          => w.write_u8(PROOF_CONV),
      ProofCmd::Refl          => w.write_u8(PROOF_REFL),
      ProofCmd::Sym           => w.write_u8(PROOF_SYMM),
      ProofCmd::Cong          => w.write_u8(PROOF_CONG),
      ProofCmd::Unfold        => w.write_u8(PROOF_UNFOLD),
      ProofCmd::ConvCut       => w.write_u8(PROOF_CONV_CUT),
      ProofCmd::ConvRef(n)    => write_cmd(w, PROOF_CONV_REF, n),
      ProofCmd::ConvSave      => w.write_u8(PROOF_CONV_SAVE),
    }
  }
}

fn write_expr_proof(w: &mut impl Write,
  heap: &[ExprNode],
  reorder: &mut Reorder,
  head: &ExprNode,
  save: bool
) -> io::Result<u32> {
  Ok(match head {
    &ExprNode::Ref(i) => match reorder.map[i] {
      None => {
        let n = write_expr_proof(w, heap, reorder, &heap[i], true)?;
        reorder.map[i] = Some(n);
        n
      }
      Some(n) => {
        ProofCmd::Ref(n.try_into().unwrap()).write_to(w)?;
        n
      }
    }
    &ExprNode::Dummy(_, s) => {
      ProofCmd::Dummy(s).write_to(w)?;
      (reorder.idx, reorder.idx += 1).0
    }
    &ExprNode::App(t, ref es) => {
      for e in es {write_expr_proof(w, heap, reorder, e, false)?;}
      if save {
        ProofCmd::TermSave(t).write_to(w)?;
        (reorder.idx, reorder.idx += 1).0
      } else {ProofCmd::Term(t).write_to(w)?; 0}
    }
  })
}

impl<'a, W: Write + Seek + ?Sized> Exporter<'a, W> {
  pub fn new(env: &'a Environment, w: &'a mut W) -> Self {
    Self {
      term_reord: TermVec(Vec::with_capacity(env.terms.len())),
      thm_reord: ThmVec(Vec::with_capacity(env.thms.len())),
      env, w, pos: 0, fixups: vec![]
    }
  }

  fn write_u32(&mut self, n: u32) -> io::Result<()> {
    WriteBytesExt::write_u32::<LE>(self, n)
  }

  fn write_u64(&mut self, n: u64) -> io::Result<()> {
    WriteBytesExt::write_u64::<LE>(self, n)
  }

  fn fixup32(&mut self) -> io::Result<Fixup32> {
    let f = Fixup32(self.pos);
    self.write_u32(0)?;
    Ok(f)
  }

  fn fixup64(&mut self) -> io::Result<Fixup64> {
    let f = Fixup64(self.pos);
    self.write_u64(0)?;
    Ok(f)
  }

  fn fixup_large(&mut self, size: usize) -> io::Result<FixupLarge> {
    let f = FixupLarge(self.pos, vec![0; size].into());
    self.write(&f.1)?;
    Ok(f)
  }

  #[inline]
  fn align_to(&mut self, n: u8) -> io::Result<u64> {
    let i = n.wrapping_sub(self.pos as u8) & (n - 1);
    self.write(&vec![0; i as usize])?;
    Ok(self.pos)
  }

  #[inline]
  fn write_sort_deps(&mut self, bound: bool, sort: SortID, deps: u64) -> io::Result<()> {
    self.write_u64(
      if bound {1} else {0} << 63 |
      (sort.0 as u64) << 56 |
      deps)
  }

  #[inline]
  fn write_term_header(header: &mut [u8], nargs: u16, sort: SortID, has_def: bool, p_term: u32) {
    LE::write_u16(&mut header[0..], nargs);
    header[2] = sort.0 | if has_def {0x80} else {0};
    LE::write_u32(&mut header[4..], p_term);
  }

  fn write_binders<T>(&mut self, args: &[(T, Type)]) -> io::Result<()> {
    let mut bv = 1;
    for (_, ty) in args {
      match ty {
        &Type::Bound(s) => {
          if bv >= (1 << 55) {panic!("more than 55 bound variables")}
          self.write_sort_deps(true, s, bv)?;
          bv *= 2;
        }
        &Type::Reg(s, deps) => self.write_sort_deps(false, s, deps)?,
      }
    }
    Ok(())
  }

  fn write_expr_unify(&mut self,
    heap: &[ExprNode],
    reorder: &mut Reorder,
    head: &ExprNode,
    save: &mut Vec<usize>
  ) -> io::Result<()> {
    macro_rules! commit {($n:expr) => {
      for i in save.drain(..) {reorder.map[i] = Some($n)}
    }}
    match head {
      &ExprNode::Ref(i) => match reorder.map[i] {
        None => {
          save.push(i);
          self.write_expr_unify(heap, reorder, &heap[i], save)
        }
        Some(n) => Ok(commit!(n)),
      }
      &ExprNode::Dummy(_, s) => {
        commit!(reorder.idx); reorder.idx += 1;
        UnifyCmd::Dummy(s).write_to(self)
      }
      &ExprNode::App(t, ref es) => {
        if save.is_empty() {
          UnifyCmd::Term(t).write_to(self)?;
        } else {
          commit!(reorder.idx); reorder.idx += 1;
          UnifyCmd::TermSave(t).write_to(self)?;
        }
        for e in es {self.write_expr_unify(heap, reorder, e, save)?}
        Ok(())
      }
    }
  }

  fn write_proof(&self, w: &mut impl Write,
    heap: &[ProofNode],
    reorder: &mut Reorder,
    hyps: &[u32],
    head: &ProofNode,
    save: bool
  ) -> io::Result<u32> {
    Ok(match head {
      &ProofNode::Ref(i) => match reorder.map[i] {
        None => {
          let n = self.write_proof(w, heap, reorder, hyps, &heap[i], true)?;
          reorder.map[i] = Some(n);
          n
        }
        Some(n) => {
          ProofCmd::Ref(n).write_to(w)?;
          n
        }
      }
      &ProofNode::Dummy(_, _) => unreachable!(),
      &ProofNode::Term {term, ref args} => {
        for e in args {self.write_proof(w, heap, reorder, hyps, e, false)?;}
        if save {
          ProofCmd::TermSave(term).write_to(w)?;
          (reorder.idx, reorder.idx += 1).0
        } else {ProofCmd::Term(term).write_to(w)?; 0}
      }
      &ProofNode::Hyp(n, _) => {
        ProofCmd::Ref(hyps[n]).write_to(w)?;
        hyps[n]
      }
      &ProofNode::Thm {thm, ref args} => {
        let t = &self.env.thms[thm];
        let nargs = t.args.len();
        let ord = &self.thm_reord[thm];
        unimplemented!()
      }
      _ => unimplemented!()
    })
  }

  #[inline]
  fn write_thm_header(header: &mut [u8], nargs: u16, p_thm: u32) {
    LE::write_u16(&mut header[0..], nargs);
    LE::write_u32(&mut header[4..], p_thm);
  }

  pub fn run(&mut self) -> io::Result<()> {
    self.write_all("MM0B".as_bytes())?; // magic
    let num_sorts = self.env.sorts.len();
    if num_sorts > 128 {panic!("too many sorts (max 128)")}
    self.write_u32(
      1 | // version
      ((num_sorts as u32) << 8) // num_sorts
    )?; // two bytes reserved
    self.write_u32(self.env.terms.len().try_into().unwrap())?; // num_terms
    self.write_u32(self.env.thms.len().try_into().unwrap())?; // num_thms
    let p_terms = self.fixup32()?;
    let p_thms = self.fixup32()?;
    let p_proof = self.fixup64()?;
    let p_index = self.fixup64()?;
    self.write_all( // sort data
      &self.env.sorts.0.iter().map(|s| {
        // 1 = PURE, 2 = STRICT, 4 = PROVABLE, 8 = FREE
        s.mods.bits()
      }).collect::<Vec<u8>>())?;

    self.align_to(8)?; p_terms.commit(self);
    let mut term_header = self.fixup_large(self.env.terms.len() * 8)?;
    for (head, t) in term_header.1.chunks_exact_mut(8).zip(&self.env.terms.0) {
      Self::write_term_header(head,
        t.args.len().try_into().expect("term has more than 65536 args"),
        t.ret.0,
        t.val.is_some(),
        self.align_to(8)?.try_into().unwrap());
      self.write_binders(&t.args)?;
      self.write_sort_deps(false, t.ret.0, t.ret.1)?;
      if let Some(val) = &t.val {
        let Expr {heap, head} = val.as_ref().unwrap_or_else(||
          panic!("def {} missing value", self.env.data[t.atom].name));
        let mut reorder = Reorder::new(
          t.args.len().try_into().unwrap(), heap.len());
        self.write_expr_unify(heap, &mut reorder, head, &mut vec![])?;
        self.write_u8(0)?;
        self.term_reord.push(Some(reorder));
      } else { self.term_reord.push(None) }
    }
    term_header.commit(self);

    self.align_to(8)?; p_thms.commit(self);
    let mut thm_header = self.fixup_large(self.env.thms.len() * 8)?;
    for (head, t) in thm_header.1.chunks_exact_mut(8).zip(&self.env.thms.0) {
      Self::write_thm_header(head,
        t.args.len().try_into().expect("theorem has more than 65536 args"),
        self.align_to(8)?.try_into().unwrap());
      self.write_binders(&t.args)?;
      let nargs = t.args.len().try_into().unwrap();
      let mut reorder = Reorder::new(nargs, t.heap.len());
      let save = &mut vec![];
      self.write_expr_unify(&t.heap, &mut reorder, &t.ret, save)?;
      for (_, h) in t.hyps.iter().rev() {
        UnifyCmd::Hyp.write_to(self)?;
        self.write_expr_unify(&t.heap, &mut reorder, h, save)?;
      }
      self.write_u8(0)?;
      self.thm_reord.push(reorder);
    }
    thm_header.commit(self);

    p_proof.commit(self);
    let mut vec = vec![];
    for s in &self.env.stmts {
      match s {
        &StmtTrace::Sort(_) => write_cmd(self, STMT_SORT, 2)?, // this takes 2 bytes
        &StmtTrace::Decl(a) => match self.env.data[a].decl.unwrap() {
          DeclKey::Term(t) => {
            let td = &self.env.terms[t];
            match &td.val {
              None => write_cmd(self, STMT_TERM, 2)?, // this takes 2 bytes
              Some(None) => unreachable!(),
              Some(Some(Expr {heap, head})) => {
                let mut reorder = Reorder::new(
                  td.args.len().try_into().unwrap(), heap.len());
                write_expr_proof(&mut vec, heap, &mut reorder, head, false)?;
                vec.write_u8(0)?;
                let cmd = STMT_DEF | if td.vis == Modifiers::LOCAL {STMT_LOCAL} else {0};
                write_cmd_bytes(self, cmd, &vec)?;
                vec.clear();
              }
            }
          }
          DeclKey::Thm(t) => {
            let td = &self.env.thms[t];
            let cmd = match &td.proof {
              None => {
                let mut reorder = Reorder::new(
                  td.args.len().try_into().unwrap(), td.heap.len());
                for (_, h) in &td.hyps {
                  write_expr_proof(&mut vec, &td.heap, &mut reorder, h, false)?;
                  ProofCmd::Hyp.write_to(&mut vec)?;
                }
                write_expr_proof(&mut vec, &td.heap, &mut reorder, &td.ret, false)?;
                STMT_AXIOM
              }
              Some(None) => panic!("proof {} missing", self.env.data[td.atom].name),
              Some(Some(Proof {heap, hyps, head})) => {
                let mut reorder = Reorder::new(
                  td.args.len().try_into().unwrap(), heap.len());
                let mut ehyps = Vec::with_capacity(hyps.len());
                for mut h in hyps {
                  while let &ProofNode::Ref(i) = h {h = &heap[i]}
                  if let ProofNode::Hyp(_, e) = h {
                    self.write_proof(&mut vec, heap, &mut reorder, &ehyps, e, false)?;
                    ProofCmd::Hyp.write_to(&mut vec)?;
                    ehyps.push(reorder.idx);
                    reorder.idx += 1;
                  } else {unreachable!()}
                }
                self.write_proof(&mut vec, &heap, &mut reorder, &ehyps, head, false)?;
                vec.write_u8(0)?;
                STMT_THM | if td.vis == Modifiers::LOCAL {STMT_LOCAL} else {0}
              }
            };
            vec.write_u8(0)?;
            write_cmd_bytes(self, cmd, &vec)?;
            vec.clear();
          }
        },
        StmtTrace::Global(_) => {}
      }
    }
    self.write_u8(0)?;
    p_index.commit_val(self, 0); // no index
    Ok(())
  }
}