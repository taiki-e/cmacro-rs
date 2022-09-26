use std::{
  fmt::Debug,
  ops::{RangeFrom, RangeTo},
};

use nom::{
  branch::alt,
  combinator::{map, opt, value},
  multi::{fold_many0, separated_list0},
  sequence::{pair, preceded, tuple},
  AsChar, Compare, FindSubstring, FindToken, IResult, InputIter, InputLength, InputTake, InputTakeAtPosition, Offset,
  ParseTo, Slice,
};
use proc_macro2::TokenStream;
use quote::{quote, TokenStreamExt};

use super::{tokens::parenthesized, *};
use crate::{CodegenContext, LocalContext, MacroArgType, UnaryOp};

/// An expression.
///
/// ```c
/// #define EXPR a + b
/// #define EXPR 1 + 2
/// ```
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub enum Expr {
  Variable { name: Identifier },
  FunctionCall(FunctionCall),
  Cast { expr: Box<Self>, ty: Type },
  Literal(Lit),
  FieldAccess { expr: Box<Self>, field: Identifier },
  Stringify(Stringify),
  Concat(Vec<Self>),
  Unary(Box<UnaryExpr>),
  Binary(Box<BinaryExpr>),
  Ternary(Box<Self>, Box<Self>, Box<Self>),
  Asm(Asm),
}

impl Expr {
  fn parse_concat<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let mut parse_string =
      alt((map(LitString::parse, |s| Self::Literal(Lit::String(s))), map(Stringify::parse, Self::Stringify)));

    let (tokens, s) = parse_string(tokens)?;

    fold_many0(
      preceded(meta, parse_string),
      move || s.clone(),
      |mut acc, item| match acc {
        Self::Concat(ref mut args) => {
          args.push(item);
          acc
        },
        acc => Self::Concat(vec![acc, item]),
      },
    )(tokens)
  }

  fn parse_factor<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    alt((
      Self::parse_concat,
      map(Lit::parse, Self::Literal),
      map(Identifier::parse, |id| Self::Variable { name: id }),
      parenthesized(Self::parse),
    ))(tokens)
  }

  fn parse_term_prec1<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, factor) = Self::parse_factor(tokens)?;

    match factor {
      Self::Variable { .. } | Self::FunctionCall(..) | Self::FieldAccess { .. } => (),
      Self::Unary(ref op) if matches!(&**op, UnaryExpr { op: UnaryOp::AddrOf, .. }) => (),
      _ => return Ok((tokens, factor)),
    }

    enum Access {
      Fn(Vec<Expr>),
      Field { field: Identifier, deref: bool },
    }

    if matches!(factor, Self::Variable { name: Identifier::Literal(ref id) } if id == "__asm") {
      if let Ok((tokens, asm)) = preceded(opt(token("volatile")), Asm::parse)(tokens) {
        return Ok((tokens, Self::Asm(asm)))
      }
    }

    let fold = fold_many0(
      alt((
        map(parenthesized(separated_list0(tuple((meta, token(","), meta)), Self::parse)), Access::Fn),
        map(pair(alt((token("."), token("->"))), Identifier::parse), |(access, field)| Access::Field {
          field,
          deref: access == "->",
        }),
      )),
      move || factor.clone(),
      |acc, access| match (acc, access) {
        (Self::Variable { name }, Access::Fn(args)) => Self::FunctionCall(FunctionCall { name, args }),
        (acc, Access::Field { field, deref }) => {
          let acc = if deref { Self::Unary(Box::new(UnaryExpr { op: UnaryOp::Deref, expr: acc })) } else { acc };

          Self::FieldAccess { expr: Box::new(acc), field }
        },
        _ => unimplemented!(),
      },
    );

    map(pair(fold, opt(alt((token("++"), token("--"))))), |(expr, op)| match op {
      Some("++") => Self::Unary(Box::new(UnaryExpr { op: UnaryOp::PostInc, expr })),
      Some("--") => Self::Unary(Box::new(UnaryExpr { op: UnaryOp::PostDec, expr })),
      _ => expr,
    })(tokens)
  }

  fn parse_term_prec2<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    alt((
      map(pair(parenthesized(Type::parse), Self::parse_term_prec2), |(ty, term)| {
        // TODO: Handle constness.
        Self::Cast { expr: Box::new(term), ty }
      }),
      map(
        pair(
          alt((
            value(UnaryOp::AddrOf, token("&")),
            value(UnaryOp::Inc, token("++")),
            value(UnaryOp::Dec, token("--")),
            value(UnaryOp::Plus, token("+")),
            value(UnaryOp::Minus, token("-")),
            value(UnaryOp::Not, token("!")),
            value(UnaryOp::Comp, token("~")),
          )),
          Self::parse_term_prec2,
        ),
        |(op, expr)| Self::Unary(Box::new(UnaryExpr { op, expr })),
      ),
      Self::parse_term_prec1,
    ))(tokens)
  }

  fn parse_term_prec3<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec2(tokens)?;

    fold_many0(
      pair(
        alt((
          map(token("*"), |_| BinaryOp::Mul),
          map(token("/"), |_| BinaryOp::Div),
          map(token("%"), |_| BinaryOp::Rem),
        )),
        Self::parse_term_prec2,
      ),
      move || term.clone(),
      |lhs, (op, rhs)| Self::Binary(Box::new(BinaryExpr { lhs, op, rhs })),
    )(tokens)
  }

  fn parse_term_prec4<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec3(tokens)?;

    fold_many0(
      pair(alt((map(token("+"), |_| BinaryOp::Add), map(token("-"), |_| BinaryOp::Sub))), Self::parse_term_prec3),
      move || term.clone(),
      |lhs, (op, rhs)| Self::Binary(Box::new(BinaryExpr { lhs, op, rhs })),
    )(tokens)
  }

  fn parse_term_prec5<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec4(tokens)?;

    fold_many0(
      pair(alt((map(token("<<"), |_| BinaryOp::Shl), map(token(">>"), |_| BinaryOp::Shr))), Self::parse_term_prec4),
      move || term.clone(),
      |lhs, (op, rhs)| Self::Binary(Box::new(BinaryExpr { lhs, op, rhs })),
    )(tokens)
  }

  fn parse_term_prec6<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec5(tokens)?;

    fold_many0(
      pair(
        alt((
          map(token("<"), |_| BinaryOp::Lt),
          map(token("<="), |_| BinaryOp::Lte),
          map(token(">"), |_| BinaryOp::Gt),
          map(token(">="), |_| BinaryOp::Gte),
        )),
        Self::parse_term_prec5,
      ),
      move || term.clone(),
      |lhs, (op, rhs)| Self::Binary(Box::new(BinaryExpr { lhs, op, rhs })),
    )(tokens)
  }

  fn parse_term_prec7<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec6(tokens)?;

    fold_many0(
      pair(alt((token("=="), token("!="))), Self::parse_term_prec6),
      move || term.clone(),
      |lhs, (op, rhs)| {
        if op == "==" {
          Self::Binary(Box::new(BinaryExpr { lhs, op: BinaryOp::Eq, rhs }))
        } else {
          Self::Binary(Box::new(BinaryExpr { lhs, op: BinaryOp::Neq, rhs }))
        }
      },
    )(tokens)
  }

  fn parse_term_prec8<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec7(tokens)?;

    fold_many0(
      preceded(token("&"), Self::parse_term_prec7),
      move || term.clone(),
      |lhs, rhs| Self::Binary(Box::new(BinaryExpr { lhs, op: BinaryOp::BitAnd, rhs })),
    )(tokens)
  }

  fn parse_term_prec9<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec8(tokens)?;

    fold_many0(
      preceded(token("^"), Self::parse_term_prec8),
      move || term.clone(),
      |lhs, rhs| Self::Binary(Box::new(BinaryExpr { lhs, op: BinaryOp::BitXor, rhs })),
    )(tokens)
  }

  fn parse_term_prec10<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec9(tokens)?;

    fold_many0(
      preceded(token("|"), Self::parse_term_prec9),
      move || term.clone(),
      |lhs, rhs| Self::Binary(Box::new(BinaryExpr { lhs, op: BinaryOp::BitOr, rhs })),
    )(tokens)
  }

  fn parse_term_prec13<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec10(tokens)?;

    // Parse ternary.
    if let Ok((tokens, _)) = token("?")(tokens) {
      let (tokens, if_branch) = Self::parse_term_prec7(tokens)?;
      let (tokens, _) = token(":")(tokens)?;
      let (tokens, else_branch) = Self::parse_term_prec7(tokens)?;
      return Ok((tokens, Self::Ternary(Box::new(term), Box::new(if_branch), Box::new(else_branch))))
    }

    Ok((tokens, term))
  }

  fn parse_term_prec14<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    let (tokens, term) = Self::parse_term_prec13(tokens)?;

    fold_many0(
      pair(
        alt((
          map(token("="), |_| BinaryOp::Assign),
          map(token("+="), |_| BinaryOp::AddAssign),
          map(token("-="), |_| BinaryOp::SubAssign),
          map(token("*="), |_| BinaryOp::MulAssign),
          map(token("/="), |_| BinaryOp::DivAssign),
          map(token("%="), |_| BinaryOp::RemAssign),
          map(token("<<="), |_| BinaryOp::ShlAssign),
          map(token(">>="), |_| BinaryOp::ShrAssign),
          map(token("&="), |_| BinaryOp::BitAndAssign),
          map(token("^="), |_| BinaryOp::BitXorAssign),
          map(token("|="), |_| BinaryOp::BitOrAssign),
        )),
        Self::parse_term_prec14,
      ),
      move || term.clone(),
      |lhs, (op, rhs)| Self::Binary(Box::new(BinaryExpr { lhs, op, rhs })),
    )(tokens)
  }

  /// Parse an expression.
  pub fn parse<I, C>(tokens: &[I]) -> IResult<&[I], Self>
  where
    I: Debug
      + InputTake
      + InputLength
      + InputIter<Item = C>
      + InputTakeAtPosition<Item = C>
      + Slice<RangeFrom<usize>>
      + Slice<RangeTo<usize>>
      + Compare<&'static str>
      + FindSubstring<&'static str>
      + ParseTo<f64>
      + ParseTo<f32>
      + Offset
      + Clone,
    C: AsChar + Copy,
    &'static str: FindToken<<I as InputIter>::Item>,
  {
    Self::parse_term_prec14(tokens)
  }

  pub(crate) fn finish<'g, C>(&mut self, ctx: &mut LocalContext<'g, C>) -> Result<Option<Type>, crate::Error>
  where
    C: CodegenContext,
  {
    match self {
      Self::Cast { expr, ty } => {
        expr.finish(ctx)?;
        ty.finish(ctx)?;
        Ok(Some(ty.clone()))
      },
      Self::Variable { ref mut name } => {
        let mut ty = name.finish(ctx)?;

        if let Identifier::Literal(id) = name {
          // Expand variable-like macro.
          if let Some(expr) = ctx.macro_variable(id.as_str()) {
            *self = expr;
            return self.finish(ctx)
          }

          if let Some(MacroArgType::Known(arg_ty)) = ctx.arg_type_mut(id.as_str()) {
            ty = Some(arg_ty.clone());
          }

          if !ctx.is_variable_known(id.as_str()) {
            // Built-in macros.
            return match id.as_str() {
              "__SCHAR_MAX__" => Ok(Some(Type::BuiltIn(BuiltInType::SChar))),
              "__SHRT_MAX__" => Ok(Some(Type::BuiltIn(BuiltInType::Short))),
              "__INT_MAX__" => Ok(Some(Type::BuiltIn(BuiltInType::Int))),
              "__LONG_MAX__" => Ok(Some(Type::BuiltIn(BuiltInType::Long))),
              "__LONG_LONG_MAX__" => Ok(Some(Type::BuiltIn(BuiltInType::LongLong))),
              _ => Err(crate::Error::UnknownVariable),
            }
          }
        }

        Ok(ty)
      },
      Self::FunctionCall(call) => call.finish(ctx),
      Self::Literal(_) => Ok(None),
      Self::FieldAccess { expr, field } => {
        expr.finish(ctx)?;
        field.finish(ctx)?;

        Ok(None)
      },
      Self::Stringify(stringify) => stringify.finish(ctx),
      Self::Concat(names) => {
        for name in names {
          name.finish(ctx)?;
        }

        // TODO: Should be `*const c_char`.
        Ok(None)
      },
      Self::Unary(op) => {
        let ty = op.finish(ctx)?;

        match (op.op, &op.expr) {
          (UnaryOp::Plus, expr @ Self::Literal(Lit::Int(_)) | expr @ Self::Literal(Lit::Float(_))) => {
            *self = expr.clone();
          },
          (UnaryOp::Minus, Self::Literal(Lit::Int(LitInt { value: i, suffix }))) => {
            let suffix = match suffix {
              Some(BuiltInType::UChar | BuiltInType::SChar) => Some(BuiltInType::SChar),
              Some(BuiltInType::UInt | BuiltInType::Int) => Some(BuiltInType::Int),
              Some(BuiltInType::ULong | BuiltInType::Long) => Some(BuiltInType::Long),
              Some(BuiltInType::ULongLong | BuiltInType::LongLong) => Some(BuiltInType::LongLong),
              _ => None,
            };
            *self = Self::Literal(Lit::Int(LitInt { value: i.wrapping_neg(), suffix }));
          },
          (UnaryOp::Minus, Self::Literal(Lit::Float(f))) => {
            *self = Self::Literal(Lit::Float(match f {
              LitFloat::Float(f) => LitFloat::Float(-f),
              LitFloat::Double(f) => LitFloat::Double(-f),
              LitFloat::LongDouble(f) => LitFloat::LongDouble(-f),
            }));
          },
          (UnaryOp::Not, Self::Literal(Lit::Int(LitInt { value: i, suffix: None }))) => {
            *self = Self::Literal(Lit::Int(LitInt { value: if *i == 0 { 1 } else { 0 }, suffix: None }));
          },
          (UnaryOp::Not, Self::Literal(Lit::Float(f))) => {
            *self = Self::Literal(Lit::Float(match f {
              LitFloat::Float(f) => LitFloat::Float(if *f == 0.0 { 1.0 } else { 0.0 }),
              LitFloat::Double(f) => LitFloat::Double(if *f == 0.0 { 1.0 } else { 0.0 }),
              LitFloat::LongDouble(f) => LitFloat::LongDouble(if *f == 0.0 { 1.0 } else { 0.0 }),
            }));
          },
          (UnaryOp::Comp, Self::Literal(Lit::Int(LitInt { value: i, suffix: None }))) => {
            *self = Self::Literal(Lit::Int(LitInt { value: !i, suffix: None }));
          },
          (UnaryOp::Comp, Self::Literal(Lit::Float(_) | Lit::String(_))) => {
            return Err(crate::Error::UnsupportedExpression)
          },
          _ => (),
        }

        Ok(ty)
      },
      Self::Binary(op) => {
        let (lhs_ty, rhs_ty) = op.finish(ctx)?;

        // Calculate numeric expression.
        match (op.op, &op.lhs, &op.rhs) {
          (BinaryOp::Mul, Self::Literal(Lit::Float(lhs)), Self::Literal(Lit::Float(rhs))) => {
            *self = Self::Literal(Lit::Float(*lhs * *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Mul, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs * *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Div, Self::Literal(Lit::Float(lhs)), Self::Literal(Lit::Float(rhs))) => {
            *self = Self::Literal(Lit::Float(*lhs / *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Div, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs / *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Rem, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs % *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Add, Self::Literal(Lit::Float(lhs)), Self::Literal(Lit::Float(rhs))) => {
            *self = Self::Literal(Lit::Float(*lhs + *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Add, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs + *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Sub, Self::Literal(Lit::Float(lhs)), Self::Literal(Lit::Float(rhs))) => {
            *self = Self::Literal(Lit::Float(*lhs - *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Sub, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs - *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Shl, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs << *rhs));
            self.finish(ctx)
          },
          (BinaryOp::Shr, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs >> *rhs));
            self.finish(ctx)
          },
          (BinaryOp::BitAnd, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs & *rhs));
            self.finish(ctx)
          },
          (BinaryOp::BitOr, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs | *rhs));
            self.finish(ctx)
          },
          (BinaryOp::BitXor, Self::Literal(Lit::Int(lhs)), Self::Literal(Lit::Int(rhs))) => {
            *self = Self::Literal(Lit::Int(*lhs ^ *rhs));
            self.finish(ctx)
          },
          (
            BinaryOp::Eq
            | BinaryOp::Neq
            | BinaryOp::And
            | BinaryOp::Or
            | BinaryOp::Lt
            | BinaryOp::Lte
            | BinaryOp::Gt
            | BinaryOp::Gte,
            _,
            _,
          ) => Ok(Some(Type::BuiltIn(BuiltInType::Bool))),
          _ => {
            if lhs_ty == rhs_ty {
              Ok(lhs_ty)
            } else {
              Ok(lhs_ty.xor(rhs_ty))
            }
          },
        }
      },
      Self::Ternary(cond, if_branch, else_branch) => {
        cond.finish(ctx)?;
        let lhs_ty = if_branch.finish(ctx)?;
        let rhs_ty = else_branch.finish(ctx)?;

        if lhs_ty == rhs_ty {
          Ok(lhs_ty)
        } else {
          Ok(lhs_ty.xor(rhs_ty))
        }
      },
      Self::Asm(asm) => asm.finish(ctx),
    }
  }

  pub(crate) fn to_tokens<C: CodegenContext>(&self, ctx: &mut LocalContext<'_, C>, tokens: &mut TokenStream) {
    match self {
      Self::Cast { ref expr, ref ty } => {
        let expr = expr.to_token_stream(ctx);

        tokens.append_all(if matches!(ty, Type::Identifier { name: Identifier::Literal(id), .. } if id == "void") {
          quote! { { drop(#expr) } }
        } else {
          let ty = ty.to_token_stream(ctx);
          quote! { #expr as #ty }
        })
      },
      Self::Variable { name: Identifier::Literal(id) } if id == "NULL" => {
        tokens.append_all(quote! { ::core::ptr::null_mut() });
      },
      Self::Variable { name: Identifier::Literal(id) } if id == "eIncrement" => {
        tokens.append_all(quote! { eNotifyAction_eIncrement });
      },
      Self::Variable { ref name } => {
        if let Identifier::Literal(id) = name {
          let prefix = &ctx.ffi_prefix();

          match id.as_str() {
            "__SCHAR_MAX__" => return tokens.append_all(quote! { #prefix c_schar::MAX }),
            "__SHRT_MAX__" => return tokens.append_all(quote! { #prefix c_short::MAX }),
            "__INT_MAX__" => return tokens.append_all(quote! { #prefix c_int::MAX }),
            "__LONG_MAX__" => return tokens.append_all(quote! { #prefix c_long::MAX }),
            "__LONG_LONG_MAX__" => return tokens.append_all(quote! { #prefix c_longlong::MAX }),
            _ => (),
          }
        }

        name.to_tokens(ctx, tokens)
      },
      Self::FunctionCall(ref call) => {
        call.to_tokens(ctx, tokens);
      },
      Self::Literal(ref lit) => lit.to_tokens(ctx, tokens),
      Self::FieldAccess { ref expr, ref field } => {
        let expr = expr.to_token_stream(ctx);
        let field = field.to_token_stream(ctx);

        tokens.append_all(quote! {
          (#expr).#field
        })
      },
      Self::Stringify(stringify) => {
        stringify.to_tokens(ctx, tokens);
      },
      Self::Concat(ref names) => {
        let names = names.iter().map(|e| e.to_token_stream(ctx)).collect::<Vec<_>>();

        tokens.append_all(quote! {
          ::core::concat!(
            #(#names),*
          )
        })
      },
      Self::Unary(op) => op.to_tokens(ctx, tokens),
      Self::Binary(op) => op.to_tokens(ctx, tokens),
      Self::Ternary(ref cond, ref if_branch, ref else_branch) => {
        let cond = cond.to_token_stream(ctx);
        let if_branch = if_branch.to_token_stream(ctx);
        let else_branch = else_branch.to_token_stream(ctx);

        tokens.append_all(quote! {

          if #cond {
            #if_branch
          } else {
            #else_branch
          }
        })
      },
      Self::Asm(ref asm) => asm.to_tokens(ctx, tokens),
    }
  }

  pub(crate) fn to_token_stream<C: CodegenContext>(&self, ctx: &mut LocalContext<'_, C>) -> TokenStream {
    let mut tokens = TokenStream::new();
    self.to_tokens(ctx, &mut tokens);
    tokens
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  macro_rules! id {
    ($name:ident) => {
      Identifier::Literal(String::from(stringify!($name)))
    };
  }

  macro_rules! var {
    ($name:ident) => {
      Expr::Variable { name: id!($name) }
    };
  }

  #[test]
  fn parse_stringify() {
    let (_, expr) = Expr::parse(&["#", "a"]).unwrap();
    assert_eq!(expr, Expr::Stringify(Stringify { id: id!(a) }));
  }

  #[test]
  fn parse_concat() {
    let (_, expr) = Expr::parse(&[r#""abc""#, r#""def""#]).unwrap();
    assert_eq!(expr, Expr::Literal(Lit::String(LitString { repr: "abcdef".into() })));

    let (_, expr) = Expr::parse(&[r#""def""#, "#", "a"]).unwrap();
    assert_eq!(
      expr,
      Expr::Concat(vec![
        Expr::Literal(Lit::String(LitString { repr: "def".into() })),
        Expr::Stringify(Stringify { id: id!(a) }),
      ])
    );
  }

  #[test]
  fn parse_access() {
    let (_, expr) = Expr::parse(&["a", ".", "b"]).unwrap();
    assert_eq!(expr, Expr::FieldAccess { expr: Box::new(var!(a)), field: id!(b) });
  }

  #[test]
  fn parse_ptr_access() {
    let (_, expr) = Expr::parse(&["a", "->", "b"]).unwrap();
    assert_eq!(
      expr,
      Expr::FieldAccess {
        expr: Box::new(Expr::Unary(Box::new(UnaryExpr { op: UnaryOp::Deref, expr: var!(a) }))),
        field: id!(b)
      }
    );
  }

  #[test]
  fn parse_assignment() {
    let (_, expr) = Expr::parse(&["a", "=", "b", "=", "c"]).unwrap();
    assert_eq!(
      expr,
      Expr::Binary(Box::new(BinaryExpr {
        lhs: var!(a),
        op: BinaryOp::Assign,
        rhs: Expr::Binary(Box::new(BinaryExpr { lhs: var!(b), op: BinaryOp::Assign, rhs: var!(c) }),),
      }))
    );
  }

  #[test]
  fn parse_function_call() {
    let (_, expr) = Expr::parse(&["my_function", "(", "arg1", ",", "arg2", ")"]).unwrap();
    assert_eq!(expr, Expr::FunctionCall(FunctionCall { name: id!(my_function), args: vec![var!(arg1), var!(arg2)] }));
  }

  #[test]
  fn parse_paren() {
    let (_, expr) = Expr::parse(&["(", "-", "123456789012ULL", ")"]).unwrap();
    assert_eq!(
      expr,
      Expr::Unary(Box::new(UnaryExpr {
        op: UnaryOp::Minus,
        expr: Expr::Literal(Lit::Int(LitInt { value: 123456789012, suffix: Some(BuiltInType::ULongLong) }))
      }))
    )
  }
}