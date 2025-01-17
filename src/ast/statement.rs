use std::fmt::Debug;

use nom::{
  branch::alt,
  combinator::{eof, map, opt, value},
  multi::many0,
  sequence::{delimited, pair, preceded, terminated, tuple},
  IResult,
};
use proc_macro2::TokenStream;
use quote::{quote, TokenStreamExt};

use super::{
  tokens::{parenthesized, punct},
  *,
};
use crate::{CodegenContext, LocalContext, MacroToken};

/// A statement.
///
/// ```c
/// #define STMT int n = 1;
/// #define STMT do { \
///   call(); \
/// } while (0)
/// ```
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub enum Statement<'t> {
  Asm(Asm<'t>),
  /// An expression.
  Expr(Expr<'t>),
  /// A function declaration.
  FunctionDecl(FunctionDecl<'t>),
  /// A variable declaration.
  VarDecl(VarDecl<'t>),
  /// A block containing multiple statements.
  Block(Vec<Self>),
  /// An if-condition.
  If {
    condition: Expr<'t>,
    if_branch: Vec<Statement<'t>>,
    else_branch: Vec<Statement<'t>>,
  },
  /// A do-while condition.
  DoWhile {
    block: Vec<Statement<'t>>,
    condition: Expr<'t>,
  },
}

impl<'t> Statement<'t> {
  fn parse_single<'i>(tokens: &'i [MacroToken<'t>]) -> IResult<&'i [MacroToken<'t>], Self> {
    let condition = |input| parenthesized(Expr::parse)(input);
    let block =
      |input| map(Self::parse_single, |stmt| if let Self::Block(stmts) = stmt { stmts } else { vec![stmt] })(input);
    let semicolon_or_eof = |input| alt((value((), punct(";")), value((), eof)))(input);

    alt((
      map(terminated(Asm::parse, semicolon_or_eof), Self::Asm),
      map(
        tuple((
          preceded(terminated(id("if"), meta), condition),
          block,
          opt(preceded(delimited(meta, id("else"), meta), block)),
        )),
        |(condition, if_branch, else_branch)| Self::If {
          condition,
          if_branch,
          else_branch: else_branch.unwrap_or_default(),
        },
      ),
      map(preceded(terminated(id("do"), meta), pair(block, preceded(id("while"), condition))), |(block, condition)| {
        Self::DoWhile { block, condition }
      }),
      map(
        delimited(terminated(punct("{"), meta), many0(preceded(meta, Self::parse_single)), preceded(meta, punct("}"))),
        Self::Block,
      ),
      map(terminated(FunctionDecl::parse, semicolon_or_eof), Self::FunctionDecl),
      map(terminated(VarDecl::parse, semicolon_or_eof), Self::VarDecl),
      map(terminated(Expr::parse, semicolon_or_eof), Self::Expr),
    ))(tokens)
  }

  /// Parse a statement.
  pub(crate) fn parse<'i>(tokens: &'i [MacroToken<'t>]) -> IResult<&'i [MacroToken<'t>], Self> {
    map(many0(delimited(meta, Self::parse_single, meta)), |mut stmts| {
      if stmts.len() == 1 {
        stmts.remove(0)
      } else {
        Self::Block(stmts)
      }
    })(tokens)
  }

  pub(crate) fn finish<C>(&mut self, ctx: &mut LocalContext<'_, 't, C>) -> Result<Option<Type<'t>>, crate::CodegenError>
  where
    C: CodegenContext,
  {
    match self {
      Self::Asm(asm) => {
        asm.finish(ctx)?;
      },
      Self::Expr(expr) => {
        expr.finish(ctx)?;
      },
      Self::FunctionDecl(f) => {
        f.finish(ctx)?;
      },
      Self::VarDecl(d) => {
        d.finish(ctx)?;
      },
      Self::Block(block) => {
        for stmt in block {
          stmt.finish(ctx)?;
        }
      },
      Self::If { condition, if_branch, else_branch } => {
        condition.finish_condition(ctx)?;

        for stmt in if_branch {
          stmt.finish(ctx)?;
        }

        for stmt in else_branch {
          stmt.finish(ctx)?;
        }
      },
      Self::DoWhile { block, condition } => {
        for stmt in block {
          stmt.finish(ctx)?;
        }

        condition.finish_condition(ctx)?;
      },
    }

    Ok(Some(Type::BuiltIn(BuiltInType::Void)))
  }

  pub(crate) fn to_tokens<C: CodegenContext>(&self, ctx: &mut LocalContext<'_, 't, C>, tokens: &mut TokenStream) {
    match self {
      Self::Asm(asm) => {
        let asm = asm.to_token_stream(ctx);
        tokens.append_all(quote! { #asm; })
      },
      Self::Expr(expr) => {
        let expr = expr.to_token_stream(ctx);
        tokens.append_all(quote! { #expr; })
      },
      Self::FunctionDecl(f) => f.to_tokens(ctx, tokens),
      Self::VarDecl(d) => {
        let decl = d.to_token_stream(ctx);
        tokens.append_all(quote! { #decl; })
      },
      Self::Block(block) => {
        let block = block.iter().map(|stmt| stmt.to_token_stream(ctx)).collect::<Vec<_>>();

        tokens.append_all(quote! {
          {
            #(#block)*
          }
        })
      },
      Self::If { condition, if_branch, else_branch } => {
        let condition = condition.to_token_stream(ctx);
        let if_branch = if_branch.iter().map(|stmt| stmt.to_token_stream(ctx)).collect::<Vec<_>>();
        let else_branch = else_branch.iter().map(|stmt| stmt.to_token_stream(ctx)).collect::<Vec<_>>();
        let else_branch = if else_branch.is_empty() { None } else { Some(quote! { else { #(#else_branch);* } }) };

        tokens.append_all(quote! {
          if #condition {
            #(#if_branch)*
          } #else_branch
        })
      },
      Self::DoWhile { block, condition } => {
        let block = block.iter().map(|stmt| stmt.to_token_stream(ctx)).collect::<Vec<_>>();
        let condition = condition.to_token_stream(ctx);

        tokens.append_all(quote! {
          loop {
            #(#block)*

            if #condition {
              continue
            }

            break
          }
        })
      },
    }
  }

  pub(crate) fn to_token_stream<C: CodegenContext>(&self, ctx: &mut LocalContext<'_, 't, C>) -> TokenStream {
    let mut tokens = TokenStream::new();
    self.to_tokens(ctx, &mut tokens);
    tokens
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parse_expr() {
    parse_tokens!(
      Statement => [id!(a), punct!("+="), lit_int!(2), punct!(";")],
      Statement::Expr(Expr::Binary(BinaryExpr {
        lhs: Box::new(var!(a)),
        op: BinaryOp::AddAssign,
        rhs: Box::new(lit!(2))
      })),
    );
  }

  #[test]
  fn parse_empty_block() {
    parse_tokens!(
      Statement => [punct!("{"), punct!("}")],
      Statement::Block(vec![]),
    );
  }

  #[test]
  fn parse_block() {
    parse_tokens!(
      Statement => [punct!("{"), id!(int), id!(a), punct!("="), lit_int!(0), punct!(";"), punct!("}")],
      Statement::Block(vec![Statement::VarDecl(VarDecl {
        ty: ty!(BuiltInType::Int),
        name: var!(a),
        rhs: lit!(0),
        is_static: false
      })]),
    );
  }

  #[test]
  fn parse_if_stmt() {
    parse_tokens!(
      Statement => [id!(if), punct!("("), id!(a), punct!(")"), id!(b), punct!(";")],
      Statement::If { condition: var!(a), if_branch: vec![Statement::Expr(var!(b))], else_branch: vec![] },
    );
  }

  #[test]
  fn parse_if_else_stmt() {
    parse_tokens!(
      Statement => [
        id!(if),
        punct!("("),
        id!(a),
        punct!(")"),
        id!(b),
        punct!(";"),
        id!(else),
        id!(c),
        punct!(";")
      ],
      Statement::If {
        condition: var!(a),
        if_branch: vec![Statement::Expr(var!(b))],
        else_branch: vec![Statement::Expr(var!(c))],
      }
    );
  }

  #[test]
  fn parse_if_block() {
    parse_tokens!(
      Statement => [
        id!(if),
        punct!("("),
        id!(a),
        punct!(")"),
        punct!("{"),
        id!(b),
        punct!(";"),
        punct!("}")
      ],
      Statement::If { condition: var!(a), if_branch: vec![Statement::Expr(var!(b))], else_branch: vec![] }
    );
  }

  #[test]
  fn parse_if_else_block() {
    parse_tokens!(
      Statement => [
        id!(if),
        punct!("("),
        id!(a),
        punct!(")"),
        punct!("{"),
        id!(b),
        punct!(";"),
        punct!("}"),
        id!(else),
        punct!("{"),
        id!(c),
        punct!(";"),
        punct!("}")
      ],
      Statement::If {
        condition: var!(a),
        if_branch: vec![Statement::Expr(var!(b))],
        else_branch: vec![Statement::Expr(var!(c))],
      }
    );
  }

  #[test]
  fn parse_do_while_stmt() {
    parse_tokens!(
      Statement => [id!(do), id!(a), punct!(";"), id!(while), punct!("("), id!(b), punct!(")")],
      Statement::DoWhile { block: vec![Statement::Expr(var!(a))], condition: var!(b) },
    );
  }

  #[test]
  fn parse_do_while_block() {
    parse_tokens!(
      Statement => [
        id!(do),
        punct!("{"),
        id!(a),
        punct!(";"),
        punct!("}"),
        id!(while),
        punct!("("),
        id!(b),
        punct!(")")
      ],
      Statement::DoWhile { block: vec![Statement::Expr(var!(a))], condition: var!(b) },
    );
  }
}
