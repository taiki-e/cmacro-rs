use std::{fmt::Debug, ops::RangeFrom};

use nom::{
  branch::alt,
  character::complete::{anychar, char},
  combinator::{all_consuming, map, map_opt, map_parser},
  multi::{fold_many0, fold_many1},
  sequence::{delimited, preceded},
  AsChar, IResult, InputIter, InputLength, Slice,
};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, TokenStreamExt};

use super::{
  literal::universal_char,
  tokens::{meta, take_one, token},
  Type,
};
use crate::{CodegenContext, Expr, Lit, LitFloat, LitInt, LocalContext, MacroArgType, ParseContext};

pub(crate) fn identifier_lit<I>(tokens: &[I]) -> IResult<&[I], LitIdent>
where
  I: Debug + InputLength + InputIter + Slice<RangeFrom<usize>> + Clone,
  <I as InputIter>::Item: AsChar,
{
  map_parser(take_one, |token| {
    map_opt(
      all_consuming(|token| {
        fold_many1(
          alt((map_opt(preceded(char('\\'), universal_char), char::from_u32), anychar)),
          Vec::new,
          |mut acc, c| {
            acc.push(c);
            acc
          },
        )(token)
      }),
      |c| {
        let s: Option<Vec<u8>> = c.iter().map(|c| if *c as u32 <= 0xff { Some(*c as u8) } else { None }).collect();
        let s =
          if let Some(s) = s.and_then(|s| String::from_utf8(s).ok()) { s } else { c.into_iter().collect::<String>() };

        let mut chars = s.chars();

        let mut start = chars.next()?;
        let mut macro_arg = false;
        let mut offset = 0;

        if start == '$' {
          start = chars.next()?;
          offset = 1;
          macro_arg = true;
        }

        if (unicode_ident::is_xid_start(start) || start == '_') && chars.all(unicode_ident::is_xid_continue) {
          Some(LitIdent { id: s[offset..].to_owned(), macro_arg })
        } else {
          None
        }
      },
    )(token)
    .map_err(|err: nom::Err<nom::error::Error<I>>| err.map_input(|_| tokens))
  })(tokens)
}

fn concat_identifier<I>(tokens: &[I]) -> IResult<&[I], LitIdent>
where
  I: Debug + InputLength + InputIter + Slice<RangeFrom<usize>> + Clone,
  <I as InputIter>::Item: AsChar,
{
  map_parser(take_one, |token| {
    all_consuming(map_opt(
      fold_many1(
        alt((map_opt(preceded(char('\\'), universal_char), char::from_u32), anychar)),
        Vec::new,
        |mut acc, c| {
          acc.push(c);
          acc
        },
      ),
      |c| {
        let s: Option<Vec<u8>> = c.iter().map(|c| if *c as u32 <= 0xff { Some(*c as u8) } else { None }).collect();
        let s =
          if let Some(s) = s.and_then(|s| String::from_utf8(s).ok()) { s } else { c.into_iter().collect::<String>() };

        let mut chars = s.chars();

        let mut start = chars.next()?;
        let mut macro_arg = false;
        let mut offset = 0;

        if start == '$' {
          start = chars.next()?;
          offset = 1;
          macro_arg = true;
        }

        if chars.all(unicode_ident::is_xid_continue) {
          Some(LitIdent { id: s[offset..].to_owned(), macro_arg })
        } else {
          None
        }
      },
    ))(token)
    .map_err(|err: nom::Err<nom::error::Error<I>>| err.map_input(|_| tokens))
  })(tokens)
}

/// A literal identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LitIdent {
  pub(crate) id: String,
  pub(crate) macro_arg: bool,
}

impl LitIdent {
  /// Get the string representation of this identifier.
  pub fn as_str(&self) -> &str {
    &self.id
  }
}

impl LitIdent {
  /// Parse an identifier.
  pub(crate) fn parse<'i, 't>(tokens: &'i [&'t str], _ctx: &ParseContext<'_>) -> IResult<&'i [&'t str], Self> {
    identifier_lit(tokens)
  }

  /// Parse an identifier.
  pub(crate) fn parse_concat<'i, 't>(tokens: &'i [&'t str], _ctx: &ParseContext<'_>) -> IResult<&'i [&'t str], Self> {
    concat_identifier(tokens)
  }
}

impl From<&str> for LitIdent {
  fn from(s: &str) -> Self {
    Self { id: s.to_owned(), macro_arg: false }
  }
}

/// An identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
  /// A literal identifier.
  ///
  /// ```c
  /// #define ID asdf
  /// ```
  Literal(LitIdent),
  /// A concatenated identifier.
  ///
  /// ```c
  /// #define ID abc ## def
  /// #define ID abc ## 123
  /// ```
  Concat(Vec<LitIdent>),
}

impl Identifier {
  /// Parse an identifier.
  pub(crate) fn parse<'i, 't>(tokens: &'i [&'t str], ctx: &ParseContext<'_>) -> IResult<&'i [&'t str], Self> {
    let (tokens, id) = map(|tokens| LitIdent::parse(tokens, ctx), Self::Literal)(tokens)?;

    fold_many0(
      preceded(delimited(meta::<&'t str>, token::<&'t str>("##"), meta::<&'t str>), |tokens| {
        LitIdent::parse_concat(tokens, ctx)
      }),
      move || id.clone(),
      |acc, item| match acc {
        Self::Literal(id) => Self::Concat(vec![id, item]),
        Self::Concat(mut ids) => {
          ids.push(item);
          Self::Concat(ids)
        },
      },
    )(tokens)
  }

  pub(crate) fn finish<C>(&mut self, ctx: &mut LocalContext<'_, C>) -> Result<Option<Type>, crate::CodegenError>
  where
    C: CodegenContext,
  {
    if let Self::Concat(ref mut ids) = self {
      unreachable!()
    }

    // An identifier does not have a type.
    Ok(None)
  }

  pub(crate) fn to_tokens<C: CodegenContext>(&self, ctx: &mut LocalContext<'_, C>, tokens: &mut TokenStream) {
    tokens.append_all(self.to_token_stream(ctx))
  }

  pub(crate) fn to_token_stream<C: CodegenContext>(&self, ctx: &mut LocalContext<'_, C>) -> TokenStream {
    match self {
      Self::Literal(ref id) => {
        if id.as_str().is_empty() {
          return quote! {}
        }

        if id.as_str() == "__VA_ARGS__" {
          return quote! { $($__VA_ARGS__),* }
        }

        let name = Ident::new(id.as_str(), Span::call_site());

        if ctx.export_as_macro && id.macro_arg {
          quote! { $#name }
        } else {
          quote! { #name }
        }
      },
      Self::Concat(ids) => unreachable!(),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const CTX: ParseContext = ParseContext::var_macro("IDENTIFIER");

  #[test]
  fn parse_literal() {
    let (_, id) = LitIdent::parse(&["asdf"], &CTX).unwrap();
    assert_eq!(id, "asdf".into());

    let (_, id) = LitIdent::parse(&["Δx"], &CTX).unwrap();
    assert_eq!(id, "Δx".into());

    let (_, id) = LitIdent::parse(&["_123"], &CTX).unwrap();
    assert_eq!(id, "_123".into());

    let (_, id) = LitIdent::parse(&["__INT_MAX__"], &CTX).unwrap();
    assert_eq!(id, "__INT_MAX__".into());
  }

  #[test]
  fn parse_wrong() {
    let res = Identifier::parse(&["123def"], &CTX);
    assert!(res.is_err());

    let res = Identifier::parse(&["123", "##", "def"], &CTX);
    assert!(res.is_err());
  }
}
