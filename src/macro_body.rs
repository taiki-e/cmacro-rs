use nom::branch::alt;
use nom::combinator::all_consuming;
use nom::combinator::map;
use nom::IResult;

use crate::{
  ast::{meta, Type},
  CodegenContext, Expr, LocalContext, Statement,
};

/// The body of a macro.
#[derive(Debug)]
pub enum MacroBody {
  Block(Statement),
  Expr(Expr),
}

impl MacroBody {
  pub fn parse<'i, 't>(input: &'i [&'t [u8]]) -> IResult<&'i [&'t [u8]], Self> {
    let (input, _) = meta(input)?;

    if input.is_empty() {
      return Ok((input, MacroBody::Block(Statement::Block(vec![]))))
    }

    let (input, body) = alt((
      all_consuming(map(Expr::parse, MacroBody::Expr)),
      all_consuming(map(Statement::parse, MacroBody::Block)),
    ))(input)?;

    Ok((input, body))
  }

  pub(crate) fn finish<'t, 'g, C>(&mut self, ctx: &mut LocalContext<'t, 'g, C>) -> Result<Option<Type>, crate::Error>
  where
    C: CodegenContext,
  {
    match self {
      Self::Block(stmt) => stmt.finish(ctx),
      Self::Expr(expr) => expr.finish(ctx),
    }
  }
}
