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
      let mut new_ids = vec![];

      let mut last_id: Option<String> = None;

      for id in ids.drain(..) {
        if id.macro_arg {
          if let Some(arg_type) = ctx.arg_type_mut(id.as_str()) {
            *arg_type = MacroArgType::Ident;

            if let Some(last_id) = last_id.take() {
              new_ids.push(LitIdent { id: last_id, macro_arg: false });
            }

            new_ids.push(id);
            continue
          }

          if let Some(arg_value) = ctx.arg_value(id.as_str()) {
            match arg_value {
              Expr::Literal(Lit::Int(LitInt { value, suffix })) => {
                let last_id = last_id.get_or_insert_with(String::new);

                last_id.push_str(&value.to_string());
                if let Some(suffix) = suffix.and_then(|s| s.suffix()) {
                  last_id.push_str(suffix);
                }
              },
              Expr::Literal(Lit::Float(LitFloat::Float(value))) => {
                let last_id = last_id.get_or_insert_with(String::new);
                last_id.push_str(&format!("{}f", value));
              },
              Expr::Literal(Lit::Float(LitFloat::Double(value))) => {
                let last_id = last_id.get_or_insert_with(String::new);
                last_id.push_str(&value.to_string());
              },
              Expr::Literal(Lit::Float(LitFloat::LongDouble(value))) => {
                let last_id = last_id.get_or_insert_with(String::new);
                last_id.push_str(&format!("{}l", value));
              },
              Expr::Variable { name: Identifier::Literal(id) } => {
                if id.macro_arg {
                  if let Some(last_id) = last_id.take() {
                    new_ids.push(LitIdent { id: last_id, macro_arg: false });
                  }

                  new_ids.push(id.clone())
                } else if let Some(ref mut last_id) = last_id {
                  last_id.push_str(id.as_str())
                } else {
                  last_id = Some(id.as_str().to_owned());
                }
              },
              Expr::Variable { name: Identifier::Concat(ids) } => {
                if let Some(last_id) = last_id.take() {
                  new_ids.push(LitIdent { id: last_id, macro_arg: false });
                }

                for id in ids {
                  new_ids.push(id.clone());
                }
              },
              _ => return Err(crate::CodegenError::UnsupportedExpression),
            }

            continue
          }

          if let Some(last_id) = last_id.take() {
            new_ids.push(LitIdent { id: last_id, macro_arg: false });
          }

          new_ids.push(id);

          continue
        }

        let last_id = last_id.get_or_insert_with(String::new);
        last_id.push_str(id.as_str());
      }

      if let Some(last_id) = last_id.take() {
        new_ids.push(LitIdent { id: last_id, macro_arg: false });
      }

      if new_ids.len() == 1 {
        *self = Self::Literal(new_ids.remove(0));
      } else {
        *ids = new_ids;
      }
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
      Self::Concat(ids) => {
        let trait_prefix = ctx.trait_prefix().into_iter();
        let ids = ids.iter().map(|id| Self::Literal(id.to_owned()).to_token_stream(ctx));
        quote! { #(#trait_prefix::)*concat_idents!(#(#ids),*) }
      },
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
