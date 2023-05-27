use std::{
  borrow::Cow,
  collections::{HashMap, HashSet},
  mem,
};

/// A set of macros.
///
/// C macros can only be fully expanded once all macros are defined.
#[derive(Debug, Clone, Default)]
pub struct MacroSet {
  var_macros: HashMap<String, Vec<String>>,
  fn_macros: HashMap<String, (Vec<String>, Vec<String>)>,
}

/// Error during macro expansion.
#[derive(Debug, PartialEq)]
pub enum ExpansionError {
  /// Macro not found.
  MacroNotFound,
  /// Open parenthesis not found.
  MissingOpenParenthesis(char),
  /// Unclosed parenthesis.
  UnclosedParenthesis(char),
  /// Function-like macro called with wrong number of arguments.
  FnMacroArgumentError {
    /// The macro name.
    name: String,
    /// The required number of arguments.
    required: usize,
    /// The given number of arguments.
    given: usize,
  },
  /// Macro starts with `##`.
  ConcatBegin,
  /// Macro ends with `##`.
  ConcatEnd,
  /// `__VA_ARGS__` used in non-variadic macro.
  NonVariadicVarArgs,
  /// Function-like macro parameter is not unique.
  NonUniqueParameter(String),
  /// `#` in function-like macro is not followed by a parameter.
  StringifyNonParameter,
  /// Concatenation does not produce a valid pre-processing token.
  InvalidConcat,
}

fn is_comment(s: &str) -> bool {
  (s.starts_with("/*") && s.ends_with("*/")) || s.starts_with("//")
}

fn is_whitespace(s: &str) -> bool {
  let s = s.trim();
  s.is_empty() || is_comment(s)
}

fn tokenize<'t>(arg_names: &[String], tokens: &'t [String]) -> Vec<Token<'t>> {
  tokens
    .iter()
    .map(|t| {
      if let Some(arg_index) = arg_names.iter().position(|arg_name| t == arg_name) {
        Token::MacroArg(arg_index)
      } else {
        match t.as_ref() {
          "__VA_ARGS__" => Token::VarArgs,
          "#" => Token::Stringify,
          "##" => Token::Concat,
          token => Token::Plain(Cow::Borrowed(token)),
        }
      }
    })
    .collect()
}

fn stringify(tokens: Vec<Token<'_>>) -> String {
  let mut s = String::new();

  let mut space_before_next = false;

  for token in tokens {
    let token = match &token {
      Token::VarArgs => "__VA_ARGS__",
      Token::Plain(t) => t.as_ref(),
      Token::NonReplacable(t) => t.as_ref(),
      Token::Stringify => "#",
      Token::Concat => "##",
      _ => {
        // At the point where `stringify` is called, macro arguments are already replaced and placemarkers removed.
        unreachable!()
      },
    };

    if token != ")" && token != "]" && token != "}" && token != "." && token != "," && token != "(" && space_before_next
    {
      s.push(' ');
    }

    space_before_next = !(token == "(" || token == "[" || token == "{" || token == ".");

    s.push_str(token)
  }

  format!("{s:?}")
}

fn detokenize<'t>(arg_names: &'t [String], tokens: Vec<Token<'t>>) -> Vec<MacroToken<'t>> {
  tokens
    .into_iter()
    .filter_map(|t| {
      Some(match t {
        Token::MacroArg(arg_index) => MacroToken::Arg(arg_index),
        Token::VarArgs => MacroToken::Arg(arg_names.len() - 1),
        Token::Plain(t) | Token::NonReplacable(t) => MacroToken::Token(t),
        Token::Stringify => MacroToken::Token(Cow::Borrowed("#")),
        Token::Concat => MacroToken::Token(Cow::Borrowed("##")),
        Token::Placemarker => return None,
      })
    })
    .collect()
}

/// A macro token.
#[derive(Debug, Clone, PartialEq)]
pub enum MacroToken<'t> {
  /// A macro parameter for the argument at the given position.
  Arg(usize),
  /// A macro token.
  Token(Cow<'t, str>),
}

#[derive(Debug, Clone, PartialEq)]
enum Token<'t> {
  MacroArg(usize),
  VarArgs,
  Plain(Cow<'t, str>),
  NonReplacable(Cow<'t, str>),
  Stringify,
  Concat,
  Placemarker,
}

impl MacroSet {
  /// Create a new macro set.
  pub fn new() -> Self {
    Self::default()
  }

  // Macros may not start or and with `##`.
  fn check_concat_begin_end(body: &[Token<'_>]) -> Result<(), ExpansionError> {
    if body.first() == Some(&Token::Concat) {
      return Err(ExpansionError::ConcatBegin)
    }

    if body.last() == Some(&Token::Concat) {
      return Err(ExpansionError::ConcatEnd)
    }

    Ok(())
  }

  fn contains_var_args(body: &[Token<'_>]) -> bool {
    body.iter().any(|t| *t == Token::VarArgs)
  }

  fn expand_macro_body<'s>(
    &'s self,
    non_replaced_names: HashSet<&str>,
    body: &[Token<'s>],
  ) -> Result<Vec<Token<'s>>, ExpansionError> {
    let mut tokens = vec![];
    let mut it = body.iter().cloned().peekable();

    while let Some(token) = it.next() {
      match token {
        Token::Plain(t) => {
          if non_replaced_names.contains(t.as_ref()) {
            tokens.push(Token::NonReplacable(t.clone()));
          } else if let Some(body) = self.var_macros.get(t.as_ref()) {
            let body = tokenize(&[], body);
            tokens.extend(self.expand_var_macro_body(non_replaced_names.clone(), t.as_ref(), &body)?);
            tokens.extend(it);
            return self.expand_macro_body(non_replaced_names, &tokens)
          } else if let Some((arg_names, body)) = self.fn_macros.get(t.as_ref()) {
            if let Ok(args) = self.collect_args(&mut it) {
              let body = tokenize(arg_names, body);
              let expanded_tokens =
                self.expand_fn_macro_body(non_replaced_names.clone(), t.as_ref(), arg_names, Some(&args), &body)?;
              tokens.extend(expanded_tokens);
              tokens.extend(it);
              return self.expand_macro_body(non_replaced_names, &tokens)
            } else {
              tokens.push(Token::Plain(t));
            }
          } else {
            tokens.push(Token::Plain(t))
          }
        },
        token => tokens.push(token),
      }
    }

    Ok(tokens)
  }

  fn expand_var_macro_body<'s, 'n>(
    &'s self,
    mut non_replaced_names: HashSet<&'n str>,
    name: &'n str,
    body: &[Token<'s>],
  ) -> Result<Vec<Token<'s>>, ExpansionError> {
    Self::check_concat_begin_end(body)?;

    // Variable-like macros shall not contain `__VA_ARGS__`.
    if Self::contains_var_args(body) {
      return Err(ExpansionError::NonVariadicVarArgs)
    }

    let mut body = Self::expand_concat(body.to_vec())?;
    Self::remove_placemarkers(&mut body);

    non_replaced_names.insert(name);

    self.expand_macro_body(non_replaced_names, &body)
  }

  fn expand_fn_macro_body<'s, 'n>(
    &'s self,
    mut non_replaced_names: HashSet<&'n str>,
    name: &'n str,
    arg_names: &[String],
    args: Option<&[Vec<Token<'s>>]>,
    body: &[Token<'s>],
  ) -> Result<Vec<Token<'s>>, ExpansionError> {
    Self::check_concat_begin_end(body)?;

    let is_variadic = arg_names.last().map(|arg_name| *arg_name == "...").unwrap_or(false);

    if !is_variadic {
      // Function-like macros shall only contain `__VA_ARGS__` if it uses ellipsis notation in the parameters.
      if Self::contains_var_args(body) {
        return Err(ExpansionError::NonVariadicVarArgs)
      }

      if let Some(args) = args {
        if arg_names.len() != args.len()
          // Allow passing an empty argument for arity 0.
          && !(arg_names.is_empty() && args.first().map(|arg| arg.is_empty()).unwrap_or(true))
        {
          return Err(ExpansionError::FnMacroArgumentError {
            name: name.to_owned(),
            required: arg_names.len(),
            given: args.len(),
          })
        }
      }
    }

    // Parameter names must be unique.
    if let Some((_, duplicate_parameter)) = arg_names
      .iter()
      .enumerate()
      .find(|(i, arg_name)| arg_names.iter().skip(i + 1).any(|arg_name2| *arg_name == arg_name2))
    {
      return Err(ExpansionError::NonUniqueParameter(duplicate_parameter.clone()))
    }

    let body = if let Some(args) = args {
      self.expand_arguments(non_replaced_names.clone(), arg_names, args, body)?
    } else {
      body.to_vec()
    };

    let mut body = Self::expand_concat(body)?;
    Self::remove_placemarkers(&mut body);

    non_replaced_names.insert(name);

    self.expand_macro_body(non_replaced_names, &body)
  }

  fn collect_args<'s, I>(&'s self, it: &mut I) -> Result<Vec<Vec<Token<'s>>>, ExpansionError>
  where
    I: Iterator<Item = Token<'s>> + Clone,
  {
    let mut parentheses = vec![]; // Keep track of parenthesis pairs.
    let mut args = vec![];
    let mut current_arg = vec![];

    let mut it2 = it.clone();

    match it2.next() {
      Some(Token::Plain(t)) if t.as_ref() == "(" => (),
      _ => return Err(ExpansionError::MissingOpenParenthesis('(')),
    }

    while let Some(token) = it2.next() {
      match token {
        Token::Plain(t) => match t.as_ref() {
          p @ "(" | p @ "[" | p @ "{" => {
            parentheses.push(p.chars().next().unwrap());
            current_arg.push(Token::Plain(t));
          },
          "}" => match parentheses.pop() {
            Some('{') => current_arg.push(Token::Plain(t)),
            Some(parenthesis) => return Err(ExpansionError::UnclosedParenthesis(parenthesis)),
            None => return Err(ExpansionError::MissingOpenParenthesis('{')),
          },
          "]" => match parentheses.pop() {
            Some('[') => current_arg.push(Token::Plain(t)),
            Some(parenthesis) => return Err(ExpansionError::UnclosedParenthesis(parenthesis)),
            None => return Err(ExpansionError::MissingOpenParenthesis('[')),
          },
          ")" => match parentheses.pop() {
            Some('(') => current_arg.push(Token::Plain(t)),
            Some(parenthesis) => return Err(ExpansionError::UnclosedParenthesis(parenthesis)),
            None => {
              args.push(mem::take(&mut current_arg));

              *it = it2;
              return Ok(args)
            },
          },
          "," if parentheses.is_empty() => {
            args.push(mem::take(&mut current_arg));
          },
          _ => current_arg.push(Token::Plain(t)),
        },
        token => current_arg.push(token),
      }
    }

    Err(ExpansionError::UnclosedParenthesis('('))
  }

  fn expand_arguments<'s>(
    &'s self,
    non_replaced_names: HashSet<&str>,
    arg_names: &[String],
    args: &[Vec<Token<'s>>],
    tokens: &[Token<'s>],
  ) -> Result<Vec<Token<'s>>, ExpansionError> {
    let mut it = tokens.iter().cloned().peekable();
    let mut tokens = vec![];

    while let Some(token) = it.next() {
      match token {
        Token::Stringify => match it.peek() {
          Some(Token::MacroArg(_) | Token::VarArgs) => {
            tokens.push(token.clone());
          },
          _ => return Err(ExpansionError::StringifyNonParameter),
        },
        Token::MacroArg(_) | Token::VarArgs => {
          let arg = if let Token::MacroArg(arg_index) = token {
            args[arg_index].clone()
          } else {
            let mut var_args = vec![];

            for (i, arg) in args[(arg_names.len() - 1)..].iter().enumerate() {
              if i > 0 {
                var_args.push(Token::Plain(Cow::Borrowed(",")));
              }
              var_args.extend(arg.clone());
            }

            var_args
          };

          match tokens.last() {
            Some(Token::Stringify) => {
              tokens.pop();
              tokens.push(Token::NonReplacable(Cow::Owned(stringify(arg))));
            },
            Some(Token::Concat) => {
              let arg = self.expand_macro_body(non_replaced_names.clone(), &arg)?;

              if arg.is_empty() {
                tokens.push(Token::Placemarker);
              } else {
                tokens.extend(arg);
              }
            },
            _ if it.peek() == Some(&Token::Concat) => {
              let arg = self.expand_macro_body(non_replaced_names.clone(), &arg)?;

              if arg.is_empty() {
                tokens.push(Token::Placemarker);
              } else {
                tokens.extend(arg);
              }
            },
            _ => tokens.extend(self.expand_macro_body(non_replaced_names.clone(), &arg)?),
          }
        },
        token => tokens.push(token),
      }
    }

    Ok(tokens)
  }

  fn expand_concat(tokens: Vec<Token<'_>>) -> Result<Vec<Token<'_>>, ExpansionError> {
    let mut it = tokens.into_iter().peekable();
    let mut tokens = vec![];

    while let Some(token) = it.next() {
      match token {
        Token::Concat
          if !matches!(tokens.last(), Some(&Token::MacroArg(_) | &Token::VarArgs))
            && !matches!(it.peek(), Some(&Token::MacroArg(_) | &Token::VarArgs)) =>
        {
          // NOTE: `##` cannot be at the beginning or end, so there must be a token before and after this.
          let lhs = tokens.pop().unwrap();
          let rhs = if it.peek() == Some(&Token::Concat) {
            // Treat consecutive `##` as one.
            Token::Placemarker
          } else {
            it.next().unwrap()
          };
          tokens.push(match (lhs, rhs) {
            (Token::Placemarker, rhs) => rhs,
            (lhs, Token::Placemarker) => lhs,
            (Token::Stringify, Token::Stringify) => Token::NonReplacable(Cow::Borrowed("##")),
            (Token::Plain(lhs) | Token::NonReplacable(lhs), Token::Plain(rhs) | Token::NonReplacable(rhs)) => {
              Token::Plain(Cow::Owned(format!("{}{}", lhs, rhs)))
            },
            _ => return Err(ExpansionError::InvalidConcat),
          })
        },
        token => tokens.push(token),
      }
    }

    Ok(tokens)
  }

  fn remove_placemarkers(tokens: &mut Vec<Token<'_>>) {
    tokens.retain(|t| *t != Token::Placemarker);
  }

  /// Define a variable-like macro.
  ///
  /// Returns true if the macro was redefined.
  pub fn define_var_macro(&mut self, name: &str, body: &[&str]) -> bool {
    if let Some(old_body) = self.var_macros.insert(name.to_owned(), body.iter().map(|&t| t.to_owned()).collect()) {
      let old_tokens = old_body.iter().filter(|t| !is_whitespace(t));
      let new_tokens = body.iter().filter(|t| !is_whitespace(t));

      !(old_tokens.zip(new_tokens).all(|(t1, t2)| t1 == t2))
    } else {
      self.fn_macros.remove(name).is_some()
    }
  }

  /// Define a function-like macro.
  ///
  /// Returns true if the macro was redefined.
  pub fn define_fn_macro(&mut self, name: &str, args: &[&str], body: &[&str]) -> bool {
    if let Some((old_args, old_body)) = self.fn_macros.insert(
      name.to_owned(),
      (args.iter().map(|&t| t.to_owned()).collect(), body.iter().map(|&t| t.to_owned()).collect()),
    ) {
      let old_tokens = old_body.iter().filter(|t| !is_whitespace(t));
      let new_tokens = body.iter().filter(|t| !is_whitespace(t));

      !(old_args.iter().zip(args.iter()).all(|(old_arg, arg)| old_arg == arg)
        && old_tokens.zip(new_tokens).all(|(t1, t2)| t1 == t2))
    } else {
      self.var_macros.remove(name).is_some()
    }
  }

  /// Undefine a macro with the given name.
  ///
  /// Returns true if the macro was undefined.
  pub fn undefine_macro(&mut self, name: &str) -> bool {
    self.var_macros.remove(name).is_some() || self.fn_macros.remove(name).is_some()
  }

  /// Expand a variable-like macro.
  pub fn expand_var_macro<'t>(&'t self, name: &str) -> Result<Vec<MacroToken<'t>>, ExpansionError> {
    let body = self.var_macros.get(name).ok_or(ExpansionError::MacroNotFound)?;
    let body = tokenize(&[], body);
    let tokens = self.expand_var_macro_body(HashSet::new(), name, &body)?;
    Ok(detokenize(&[], tokens))
  }

  pub fn fn_macro_args<'s>(&'s self, name: &str) -> Option<&'s [String]> {
    self.fn_macros.get(name).map(|(arg_names, _)| arg_names.as_slice())
  }

  /// Expand a function-like macro.
  pub fn expand_fn_macro<'t>(&'t self, name: &str) -> Result<Vec<MacroToken<'t>>, ExpansionError> {
    let (arg_names, body) = self.fn_macros.get(name).ok_or(ExpansionError::MacroNotFound)?;
    let body = tokenize(arg_names, body);
    let tokens = self.expand_fn_macro_body(HashSet::new(), name, arg_names, None, &body)?;
    Ok(detokenize(arg_names, tokens))
  }
}

pub(crate) trait ToMacroToken<'t> {
  fn to_macro_token(self) -> MacroToken<'t>;
}

impl<'t> ToMacroToken<'t> for MacroToken<'t> {
  fn to_macro_token(self) -> MacroToken<'t> {
    self
  }
}

impl<'t> ToMacroToken<'t> for &'t str {
  fn to_macro_token(self) -> MacroToken<'t> {
    MacroToken::Token(Cow::Borrowed(self))
  }
}

macro_rules! arg {
  ($index:expr) => {{
    MacroToken::Arg($index)
  }};
}
pub(crate) use arg;

macro_rules! tokens {
  ($($token:expr),*) => {{
    use $crate::macro_set::ToMacroToken;

    &[
      $(
        $token.to_macro_token()
      ),*
    ]
  }};
}
pub(crate) use tokens;

macro_rules! token_vec {
  ($($token:expr),*) => {{
    use $crate::macro_set::ToMacroToken;

    vec![
      $(
        $token.to_macro_token()
      ),*
    ]
  }};
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn macro_set() {
    let mut macro_set = MacroSet::new();

    macro_set.define_var_macro("VAR", &["A", "+", "B"]);
    macro_set.define_var_macro("A", &["2"]);
    macro_set.define_var_macro("B", &["3"]);
    macro_set.define_fn_macro("PLUS", &["A", "B"], &["A", "+", "B"]);
    macro_set.define_fn_macro("F1", &["A", "B"], &["A", "+", "VAR", "+", "B"]);
    macro_set.define_var_macro("PLUS_VAR", &["PLUS", "(", "7", ",", "8", ")"]);
    macro_set.define_var_macro("PLUS_PLUS_VAR", &["PLUS", "(", "PLUS", "(", "3", ",", "1", ")", ",", "8", ")"]);
    macro_set.define_var_macro("PLUS_VAR_VAR", &["PLUS", "(", "7", ",", "VAR", ")"]);

    assert_eq!(macro_set.expand_var_macro("VAR"), Ok(token_vec!["2", "+", "3"]));
    assert_eq!(macro_set.expand_fn_macro("F1"), Ok(token_vec![arg!(0), "+", "2", "+", "3", "+", arg!(1)]));
    assert_eq!(macro_set.expand_var_macro("PLUS_VAR"), Ok(token_vec!["7", "+", "8"]));
    assert_eq!(macro_set.expand_var_macro("PLUS_PLUS_VAR"), Ok(token_vec!["3", "+", "1", "+", "8"]));
    assert_eq!(macro_set.expand_var_macro("PLUS_VAR_VAR"), Ok(token_vec!["7", "+", "2", "+", "3"]));
  }

  #[test]
  fn parse_disjunct() {
    let mut macro_set = MacroSet::new();

    macro_set.define_var_macro("THREE_PLUS", &["3", "+"]);
    macro_set.define_var_macro("FOUR", &["4"]);
    macro_set.define_var_macro("THREE_PLUS_FOUR", &["THREE_PLUS", "FOUR"]);

    assert_eq!(macro_set.expand_var_macro("THREE_PLUS_FOUR"), Ok(token_vec!["3", "+", "4"]));
  }

  #[test]
  fn parse_fn_no_args() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("FUNC", &[], &["123"]);
    macro_set.define_var_macro("ONE_TWO_THREE", &["FUNC", "(", ")"]);
    assert_eq!(macro_set.expand_var_macro("ONE_TWO_THREE"), Ok(token_vec!["123"]));
  }

  #[test]
  fn parse_disjunct_fn() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("FUNC1", &["a", "b"], &["a", "+", "b"]);
    macro_set.define_var_macro("FUNC1_PARTIAL", &["FUNC1", "(", "1", ","]);
    macro_set.define_fn_macro("FUNC2", &[], &["FUNC1_PARTIAL", "2", ")"]);

    assert_eq!(macro_set.expand_fn_macro("FUNC2"), Ok(token_vec!["1", "+", "2"]));
  }

  #[test]
  fn parse_disjunct_fn_call() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("BAR", &["a", "b"], &["a", "+", "b"]);
    macro_set.define_fn_macro("FOO", &[], &["BAR"]);
    macro_set.define_var_macro("APLUSB", &["FOO", "(", ")", "(", "3", ",", "1", ")"]);

    assert_eq!(macro_set.expand_var_macro("APLUSB"), Ok(token_vec!["3", "+", "1"]));
  }

  #[test]
  fn parse_recursive() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("FUNC1", &["arg"], &["FUNC2", "(", "arg", ")"]);
    macro_set.define_fn_macro("FUNC2", &["arg"], &["FUNC1", "(", "arg", ")"]);
    macro_set.define_var_macro("VAR1", &["1", "+", "VAR1"]);
    assert_eq!(macro_set.expand_fn_macro("FUNC1"), Ok(token_vec!["FUNC1", "(", arg!(0), ")"]));
    assert_eq!(macro_set.expand_fn_macro("FUNC2"), Ok(token_vec!["FUNC2", "(", arg!(0), ")"]));
    assert_eq!(macro_set.expand_var_macro("VAR1"), Ok(token_vec!["1", "+", "VAR1"]));
  }

  #[test]
  fn parse_stringify() {
    let mut macro_set = MacroSet::new();

    macro_set.define_var_macro("s", &["377"]);
    macro_set.define_fn_macro("STRINGIFY", &["s"], &["#", "s"]);
    assert_eq!(macro_set.expand_fn_macro("STRINGIFY"), Ok(token_vec!["#", arg!(0)]));
  }

  #[test]
  fn parse_stringify_nested() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("STRINGIFY", &["s"], &["#", "s"]);
    macro_set.define_var_macro("s", &["STRINGIFY", "(", "asdf", ")"]);
    macro_set.define_var_macro("e", &["STRINGIFY", "(", "a", "+", "b", ")"]);
    assert_eq!(macro_set.expand_var_macro("s"), Ok(token_vec!["\"asdf\""]));
    assert_eq!(macro_set.expand_var_macro("e"), Ok(token_vec!["\"a + b\""]));
  }

  #[test]
  fn parse_stringify_var_args() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("STRINGIFY", &["..."], &["#", "__VA_ARGS__"]);
    macro_set.define_var_macro("ZERO", &["STRINGIFY", "(", ")"]);
    macro_set.define_var_macro("ONE", &["STRINGIFY", "(", "asdf", ")"]);
    macro_set.define_var_macro("TWO", &["STRINGIFY", "(", "a", ",", "b", ")"]);
    assert_eq!(macro_set.expand_var_macro("ZERO"), Ok(token_vec!["\"\""]));
    assert_eq!(macro_set.expand_var_macro("ONE"), Ok(token_vec!["\"asdf\""]));
    assert_eq!(macro_set.expand_var_macro("TWO"), Ok(token_vec!["\"a, b\""]));
  }

  #[test]
  fn parse_concat() {
    let mut macro_set = MacroSet::new();

    macro_set.define_var_macro("A", &["1"]);
    macro_set.define_var_macro("B", &["2"]);
    macro_set.define_var_macro("CONCAT", &["A", "##", "B"]);
    assert_eq!(macro_set.expand_var_macro("CONCAT"), Ok(token_vec!["AB"]));
  }

  #[test]
  fn parse_concat_string() {
    let mut macro_set = MacroSet::new();

    macro_set.define_var_macro("A", &["1"]);
    macro_set.define_var_macro("B", &["2"]);
    macro_set.define_var_macro("C", &["\", world!\""]);
    macro_set.define_var_macro("AB", &["\"Hello\""]);
    macro_set.define_fn_macro("CONCAT_STRING", &["A", "B"], &["A", "##", "B", "C"]);
    assert_eq!(macro_set.expand_fn_macro("CONCAT_STRING"), Ok(token_vec![arg!(0), "##", arg!(1), "\", world!\""]));
  }

  #[test]
  fn parse_concat_empty() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("CONCAT", &["a", "b"], &["a", "##", "b"]);
    macro_set.define_var_macro("EMPTY", &["CONCAT", "(", ",", ")"]);
    assert_eq!(macro_set.expand_var_macro("EMPTY"), Ok(token_vec![]));
  }

  #[test]
  fn parse_c_std_6_10_3_3_example() {
    let mut macro_set = MacroSet::new();

    macro_set.define_var_macro("hash_hash", &["#", "##", "#"]);
    macro_set.define_fn_macro("mkstr", &["a"], &["#", "a"]);
    macro_set.define_fn_macro("in_between", &["a"], &["mkstr", "(", "a", ")"]);
    macro_set.define_fn_macro("join", &["c", "d"], &["in_between", "(", "c", "hash_hash", "d", ")"]);
    macro_set.define_var_macro("join_x_y", &["join", "(", "x", ",", "y", ")"]);
    assert_eq!(macro_set.expand_var_macro("join_x_y"), Ok(token_vec!["\"x ## y\""]));
  }

  #[test]
  fn parse_c_std_6_10_3_5_example_3() {
    let mut macro_set = MacroSet::new();

    macro_set.define_var_macro("x", &["3"]);
    macro_set.define_fn_macro("f", &["a"], &["f", "(", "x", "*", "(", "a", ")", ")"]);
    macro_set.define_var_macro("x", &["2"]);
    macro_set.define_var_macro("g", &["f"]);
    macro_set.define_var_macro("z", &["z", "[", "0", "]"]);
    macro_set.define_var_macro("h", &["g", "(", "~"]);
    macro_set.define_fn_macro("m", &["a"], &["a", "(", "w", ")"]);
    macro_set.define_var_macro("w", &["0", ",", "1"]);
    macro_set.define_fn_macro("t", &["a"], &["a"]);
    macro_set.define_fn_macro("p", &[], &["int"]);
    macro_set.define_fn_macro("q", &["x"], &["x"]);
    macro_set.define_fn_macro("r", &["x", "y"], &["x", "##", "y"]);
    macro_set.define_fn_macro("str", &["x"], &["#", "x"]);

    macro_set.define_var_macro(
      "line1",
      &[
        "f", "(", "y", "+", "1", ")", "+", "f", "(", "f", "(", "z", ")", ")", "%", "t", "(", "t", "(", "g", ")", "(",
        "0", ")", "+", "t", ")", "(", "1", ")", ";",
      ],
    );
    macro_set.define_var_macro(
      "line2",
      &[
        "g", "(", "x", "+", "(", "3", ",", "4", ")", "-", "w", ")", "|", "h", "5", ")", "&", "m", "(", "f", ")", "^",
        "m", "(", "m", ")", ";",
      ],
    );
    macro_set.define_var_macro(
      "line3",
      &[
        "p", "(", ")", "i", "[", "q", "(", ")", "]", "=", "{", "q", "(", "1", ")", ",", "r", "(", "2", ",", "3", ")",
        ",", "r", "(", "4", ",", ")", ",", "r", "(", ",", "5", ")", ",", "r", "(", ",", ")", "}", ";",
      ],
    );
    macro_set.define_var_macro(
      "line4",
      &["char", "c", "[", "2", "]", "[", "6", "]", "=", "{", "str", "(", "hello", ")", ",", "str", "(", ")", "}", ";"],
    );

    assert_eq!(
      macro_set.expand_var_macro("line1"),
      Ok(token_vec![
        "f", "(", "2", "*", "(", "y", "+", "1", ")", ")", "+", "f", "(", "2", "*", "(", "f", "(", "2", "*", "(", "z",
        "[", "0", "]", ")", ")", ")", ")", "%", "f", "(", "2", "*", "(", "0", ")", ")", "+", "t", "(", "1", ")", ";"
      ])
    );
    assert_eq!(
      macro_set.expand_var_macro("line2"),
      Ok(token_vec![
        "f", "(", "2", "*", "(", "2", "+", "(", "3", ",", "4", ")", "-", "0", ",", "1", ")", ")", "|", "f", "(", "2",
        "*", "(", "~", "5", ")", ")", "&", "f", "(", "2", "*", "(", "0", ",", "1", ")", ")", "^", "m", "(", "0", ",",
        "1", ")", ";"
      ])
    );
    assert_eq!(
      macro_set.expand_var_macro("line3"),
      Ok(token_vec!["int", "i", "[", "]", "=", "{", "1", ",", "23", ",", "4", ",", "5", ",", "}", ";"])
    );
    assert_eq!(
      macro_set.expand_var_macro("line4"),
      Ok(token_vec!["char", "c", "[", "2", "]", "[", "6", "]", "=", "{", "\"hello\"", ",", "\"\"", "}", ";"])
    );
  }

  #[test]
  fn parse_c_std_6_10_3_5_example_4() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("str", &["s"], &["#", "s"]);
    macro_set.define_fn_macro("xstr", &["s"], &["str", "(", "s", ")"]);
    macro_set.define_fn_macro(
      "debug",
      &["s", "t"],
      &[
        "printf",
        "(",
        "\"x\"",
        "#",
        "s",
        "\"= %d, x\"",
        "#",
        "t",
        "\"= %s\"",
        ",",
        "x",
        "##",
        "s",
        ",",
        "x",
        "##",
        "t",
        ")",
      ],
    );
    macro_set.define_fn_macro("INCFILE", &["n"], &["vers", "##", "n"]);
    macro_set.define_fn_macro("glue", &["a", "b"], &["a", "##", "b"]);
    macro_set.define_fn_macro("xglue", &["a", "b"], &["glue", "(", "a", ",", "b", ")"]);
    macro_set.define_var_macro("HIGHLOW", &["\"hello\""]);
    macro_set.define_var_macro("LOW", &["LOW", "\", world\""]);

    macro_set.define_var_macro("line1", &["debug", "(", "1", ",", "2", ")", ";"]);
    macro_set.define_var_macro(
      "line2",
      &[
        "fputs",
        "(",
        "str",
        "(",
        "strncmp",
        "(",
        "\"abc\0d\"",
        ",",
        "\"abc\"",
        ",",
        "'\\4'",
        ")", // this goes away
        "==",
        "0",
        ")",
        "str",
        "(",
        ":",
        "@",
        "\\n",
        ")",
        ",",
        "s",
        ")",
        ";",
      ],
    );
    macro_set.define_var_macro("line3", &["#include", "xstr", "(", "INCFILE", "(", "2", ")", ".", "h", ")"]);

    assert_eq!(
      macro_set.expand_var_macro("line1"),
      Ok(token_vec![
        "printf",
        "(",
        "\"x\"",
        "\"1\"",
        "\"= %d, x\"",
        "\"2\"",
        "\"= %s\"",
        ",",
        "x1",
        ",",
        "x2",
        ")",
        ";"
      ])
    );
    assert_eq!(
      macro_set.expand_var_macro("line2"),
      Ok(token_vec![
        "fputs",
        "(",
        "\"strncmp(\\\"abc\\0d\\\", \\\"abc\\\", '\\\\4') == 0\"",
        "\": @ \\\\n\"",
        ",",
        "s",
        ")",
        ";"
      ])
    );
    assert_eq!(macro_set.expand_var_macro("line3"), Ok(token_vec!["#include", "\"vers2.h\""]));
  }

  #[test]
  fn parse_c_std_6_10_3_5_example_5() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("t", &["x", "y", "z"], &["x", "##", "y", "##", "z"]);
    macro_set.define_var_macro(
      "line1",
      &[
        "int", "j", "[", "]", "=", "{", //
        "t", "(", "1", ",", "2", ",", "3", ")", ",", //
        "t", "(", ",", "4", ",", "5", ")", ",", //
        "t", "(", "6", ",", ",", "7", ")", ",", //
        "t", "(", "8", ",", "9", ",", ")", ",", //
        "t", "(", "10", ",", ",", ")", ",", //
        "t", "(", ",", "11", ",", ")", ",", //
        "t", "(", ",", ",", "12", ")", ",", //
        "t", "(", ",", ",", ")", //
        "}", ";",
      ],
    );

    assert_eq!(
      macro_set.expand_var_macro("line1"),
      Ok(token_vec![
        "int", "j", "[", "]", "=", "{", "123", ",", "45", ",", "67", ",", "89", ",", "10", ",", "11", ",", "12", ",",
        "}", ";"
      ])
    );
  }

  #[test]
  fn parse_c_std_6_10_3_5_example_6() {
    let mut macro_set = MacroSet::new();

    macro_set.define_var_macro("OBJ_LIKE", &["/* whie space */", "(", "1", "-", "1", ")", "/* other */"]);
    assert!(!macro_set.define_var_macro("OBJ_LIKE", &["(", "1", "-", "1", ")"]));

    assert!(!macro_set.define_fn_macro("FUNC_LIKE", &["a"], &["(", "a", ")"]));
    assert!(!macro_set.define_fn_macro(
      "FUNC_LIKE",
      &["a"],
      &["(", "/* note the white space */", "a", "/* other stuff on this line \n */", ")"]
    ));

    let mut macro_set = MacroSet::new();

    macro_set.define_var_macro("OBJ_LIKE", &["(", "0", ")"]);
    assert!(macro_set.define_var_macro("OBJ_LIKE", &["(", "1", "-", "1", ")"]));

    macro_set.define_fn_macro("FUNC_LIKE", &["b"], &["(", "a", ")"]);
    assert!(macro_set.define_fn_macro("FUNC_LIKE", &["b"], &["(", "b", ")"]));
  }

  #[test]
  fn parse_c_std_6_10_3_5_example_7() {
    let mut macro_set = MacroSet::new();

    macro_set.define_fn_macro("debug", &["..."], &["fprintf", "(", "stderr", ",", "__VA_ARGS__", ")"]);
    macro_set.define_fn_macro("showlist", &["..."], &["puts", "(", "#", "__VA_ARGS__", ")"]);
    macro_set.define_fn_macro(
      "report",
      &["test", "..."],
      &["(", "(", "test", ")", "?", "puts", "(", "#", "test", ")", ":", "printf", "(", "__VA_ARGS__", ")", ")"],
    );

    macro_set.define_var_macro("line1", &["debug", "(", "\"Flag\"", ")", ";"]);
    macro_set.define_var_macro("line2", &["debug", "(", "\"X = %d\\n\"", ",", "x", ")", ";"]);
    macro_set.define_var_macro(
      "line3",
      &["showlist", "(", "The", "first", ",", "second", ",", "and", "third", "items", ".", ")", ";"],
    );
    macro_set.define_var_macro(
      "line4",
      &["report", "(", "x", ">", "y", ",", "\"x is %d but y is %d\"", ",", "x", ",", "y", ")", ";"],
    );

    assert_eq!(
      macro_set.expand_var_macro("line1"),
      Ok(token_vec!["fprintf", "(", "stderr", ",", "\"Flag\"", ")", ";"])
    );
    assert_eq!(
      macro_set.expand_var_macro("line2"),
      Ok(token_vec!["fprintf", "(", "stderr", ",", "\"X = %d\\n\"", ",", "x", ")", ";"])
    );
    assert_eq!(
      macro_set.expand_var_macro("line3"),
      Ok(token_vec!["puts", "(", "\"The first, second, and third items.\"", ")", ";"])
    );
    assert_eq!(
      macro_set.expand_var_macro("line4"),
      Ok(token_vec![
        "(",
        "(",
        "x",
        ">",
        "y",
        ")",
        "?",
        "puts",
        "(",
        "\"x > y\"",
        ")",
        ":",
        "printf",
        "(",
        "\"x is %d but y is %d\"",
        ",",
        "x",
        ",",
        "y",
        ")",
        ")",
        ";"
      ])
    );
  }
}
