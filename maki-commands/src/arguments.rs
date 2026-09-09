use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticArgumentKind {
    String,
    Integer,
    Enum(&'static [&'static str]),
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticPositionalArgument {
    pub name: &'static str,
    pub kind: StaticArgumentKind,
    pub optional: bool,
    pub variadic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticCommandArguments {
    Legacy(crate::spec::ArgumentArity),
    Positional(&'static [StaticPositionalArgument]),
}

impl StaticPositionalArgument {
    pub const fn optional(name: &'static str, kind: StaticArgumentKind) -> Self {
        Self {
            name,
            kind,
            optional: true,
            variadic: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentKind {
    String,
    Integer,
    Enum(Arc<[Arc<str>]>),
    EnumWithDefault {
        choices: Arc<[Arc<str>]>,
        default: Arc<str>,
    },
    File,
    Directory,
}

impl From<StaticArgumentKind> for ArgumentKind {
    fn from(kind: StaticArgumentKind) -> Self {
        match kind {
            StaticArgumentKind::String => Self::String,
            StaticArgumentKind::Integer => Self::Integer,
            StaticArgumentKind::Enum(choices) => {
                Self::Enum(choices.iter().copied().map(Arc::from).collect())
            }
            StaticArgumentKind::File => Self::File,
            StaticArgumentKind::Directory => Self::Directory,
        }
    }
}

impl From<StaticPositionalArgument> for PositionalArgument {
    fn from(argument: StaticPositionalArgument) -> Self {
        Self {
            name: Arc::from(argument.name),
            kind: argument.kind.into(),
            optional: argument.optional,
            variadic: argument.variadic,
            completion: CompletionPolicy::Default,
        }
    }
}

impl ArgumentKind {
    pub fn enum_with_default(
        choices: impl Into<Arc<[Arc<str>]>>,
        default: impl Into<Arc<str>>,
    ) -> Self {
        Self::EnumWithDefault {
            choices: choices.into(),
            default: default.into(),
        }
    }

    pub fn enum_choices(&self) -> Option<&[Arc<str>]> {
        match self {
            Self::Enum(choices) | Self::EnumWithDefault { choices, .. } => Some(choices),
            _ => None,
        }
    }

    pub fn default_value(&self) -> Option<&str> {
        match self {
            Self::EnumWithDefault { default, .. } => Some(default),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Enum(_) | Self::EnumWithDefault { .. } => "enum",
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CompletionPolicy {
    #[default]
    Default,
    Disabled,
    Replace,
    Extend,
}

impl CompletionPolicy {
    pub fn compose<T>(self, defaults: Vec<T>, provider: Vec<T>) -> Vec<T>
    where
        T: Eq + std::hash::Hash + Clone,
    {
        match self {
            Self::Disabled => Vec::new(),
            Self::Replace => provider,
            Self::Default => {
                if provider.is_empty() {
                    defaults
                } else {
                    provider
                }
            }
            Self::Extend => {
                let provider_values: std::collections::HashSet<_> =
                    provider.iter().cloned().collect();
                defaults
                    .into_iter()
                    .filter(|item| !provider_values.contains(item))
                    .chain(provider)
                    .collect()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionalArgument {
    pub name: Arc<str>,
    pub kind: ArgumentKind,
    pub optional: bool,
    pub variadic: bool,
    pub completion: CompletionPolicy,
}

impl PositionalArgument {
    pub fn required(name: impl Into<Arc<str>>, kind: ArgumentKind) -> Self {
        Self {
            name: name.into(),
            kind,
            optional: false,
            variadic: false,
            completion: CompletionPolicy::Default,
        }
    }

    pub fn optional(name: impl Into<Arc<str>>, kind: ArgumentKind) -> Self {
        Self {
            name: name.into(),
            kind,
            optional: true,
            variadic: false,
            completion: CompletionPolicy::Default,
        }
    }

    pub fn with_completion(mut self, completion: CompletionPolicy) -> Self {
        self.completion = completion;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandArguments {
    Legacy(crate::spec::ArgumentArity),
    Positional(Arc<[PositionalArgument]>),
}

impl CommandArguments {
    pub fn arity(&self) -> crate::spec::ArgumentArity {
        match self {
            Self::Legacy(arity) => *arity,
            Self::Positional(arguments) => {
                let mut min = 0;
                let mut max = Some(0usize);
                for argument in arguments.iter() {
                    if !argument.optional {
                        min += 1;
                    }
                    max = if argument.variadic {
                        None
                    } else {
                        max.map(|value| value + 1)
                    };
                }
                crate::spec::ArgumentArity { min, max }
            }
        }
    }

    pub fn positional(&self) -> Option<&[PositionalArgument]> {
        match self {
            Self::Positional(arguments) => Some(arguments),
            Self::Legacy(_) => None,
        }
    }

    pub fn usage_hint(&self) -> Option<Arc<str>> {
        let Self::Positional(arguments) = self else {
            return None;
        };
        if arguments.is_empty() {
            return None;
        }
        let mut hint = String::new();
        for (index, argument) in arguments.iter().enumerate() {
            if index != 0 {
                hint.push(' ');
            }
            if argument.optional {
                hint.push('[');
            } else {
                hint.push('<');
            }
            hint.push_str(&argument.name);
            if argument.optional {
                hint.push(']');
            } else {
                hint.push('>');
            }
            if argument.variadic {
                hint.push_str("...");
            }
        }
        Some(Arc::from(hint))
    }

    pub fn accepts(&self, count: usize) -> bool {
        self.arity().accepts(count)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToken {
    pub value: Arc<str>,
    pub range: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentValue {
    String(Arc<str>),
    Integer(i64),
    Enum(Arc<str>),
    File(PathBuf),
    Directory(PathBuf),
}

impl ArgumentValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) | Self::Enum(value) => Some(value),
            Self::File(value) | Self::Directory(value) => value.to_str(),
            Self::Integer(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgument {
    pub name: Arc<str>,
    pub values: Arc<[ArgumentValue]>,
    pub variadic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArguments {
    tokens: Arc<[ParsedToken]>,
    arguments: Arc<[ParsedArgument]>,
}

impl ParsedArguments {
    pub fn tokens(&self) -> &[ParsedToken] {
        &self.tokens
    }

    pub fn arguments(&self) -> &[ParsedArgument] {
        &self.arguments
    }

    pub fn get(&self, name: &str) -> Option<&ArgumentValue> {
        self.arguments
            .iter()
            .find(|argument| argument.name.as_ref() == name)?
            .values
            .first()
    }

    pub fn variadic(&self, name: &str) -> Option<&[ArgumentValue]> {
        self.arguments
            .iter()
            .find(|argument| argument.name.as_ref() == name)
            .map(|argument| argument.values.as_ref())
    }

    pub(crate) fn new(tokens: Vec<ParsedToken>, arguments: Vec<ParsedArgument>) -> Self {
        Self {
            tokens: tokens.into(),
            arguments: arguments.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub message: Arc<str>,
    pub span: std::ops::Range<usize>,
}

impl fmt::Display for LexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at bytes {}..{}",
            self.message, self.span.start, self.span.end
        )
    }
}

impl std::error::Error for LexError {}

pub fn lex_strict(input: &str) -> Result<Vec<ParsedToken>, LexError> {
    lex(input, false)
}

pub fn lex_tolerant(input: &str) -> Result<Vec<ParsedToken>, LexError> {
    lex(input, true)
}

fn lex(input: &str, tolerant: bool) -> Result<Vec<ParsedToken>, LexError> {
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < input.len() {
        while index < input.len() {
            let character = input[index..].chars().next().unwrap();
            if !character.is_whitespace() {
                break;
            }
            index += character.len_utf8();
        }
        if index == input.len() {
            break;
        }
        let start = index;
        let mut value = String::new();
        let mut quote = None;
        while index < input.len() {
            let character = input[index..].chars().next().unwrap();
            match quote {
                Some('"') => {
                    if character == '"' {
                        quote = None;
                        index += 1;
                        continue;
                    }
                    if character == '\\' {
                        let slash = index;
                        index += 1;
                        if index == input.len() {
                            value.push('\\');
                            if !tolerant {
                                return Err(LexError {
                                    message: Arc::from("unterminated double quote"),
                                    span: slash..index,
                                });
                            }
                            break;
                        }
                        let escaped = input[index..].chars().next().unwrap();
                        if matches!(escaped, '"' | '\\') {
                            value.push(escaped);
                            index += escaped.len_utf8();
                        } else {
                            value.push('\\');
                            index += escaped.len_utf8();
                        }
                    } else {
                        value.push(character);
                        index += character.len_utf8();
                    }
                }
                Some('\'') => {
                    if character == '\'' {
                        quote = None;
                        index += 1;
                        continue;
                    }
                    value.push(character);
                    index += character.len_utf8();
                }
                None => {
                    if character.is_whitespace() {
                        break;
                    }
                    if matches!(character, '\'' | '"') {
                        quote = Some(character);
                        index += 1;
                        continue;
                    }
                    value.push(character);
                    index += character.len_utf8();
                }
                _ => unreachable!(),
            }
        }
        if quote.is_some() && !tolerant {
            return Err(LexError {
                message: Arc::from("unterminated quote"),
                span: start..index,
            });
        }
        if value.as_bytes().contains(&0) {
            return Err(LexError {
                message: Arc::from("NUL is not allowed in arguments"),
                span: start..index,
            });
        }
        tokens.push(ParsedToken {
            value: Arc::from(value),
            range: start..index,
        });
    }
    Ok(tokens)
}

pub const MAX_EXACT_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ArgumentParseError {
    #[error("argument {index} ({name}) expected {expected}: {message}")]
    Invalid {
        index: usize,
        name: Arc<str>,
        expected: Arc<str>,
        message: Arc<str>,
        span: std::ops::Range<usize>,
    },
    #[error("expected {expected} arguments, got {actual}")]
    Count {
        expected: crate::spec::ArgumentArity,
        actual: usize,
    },
    #[error("{0}")]
    Lex(#[from] LexError),
}

pub fn parse_completion_prefix(
    input: &str,
    schema: &[PositionalArgument],
    argument_index: usize,
) -> Arc<[ArgumentValue]> {
    parse_completion_prefix_arguments(input, schema, argument_index)
        .iter()
        .flat_map(|argument| argument.values.iter().cloned())
        .collect()
}

pub fn parse_completion_prefix_arguments(
    input: &str,
    schema: &[PositionalArgument],
    argument_index: usize,
) -> Arc<[ParsedArgument]> {
    let Ok(tokens) = lex_tolerant(input) else {
        return Arc::from([]);
    };
    // The active query is the token being completed. With a trailing
    // whitespace the cursor is in an empty slot after all complete tokens, so
    // every token is a preceding value. Without it, the last token is the
    // partial active query and everything before it is preceding. The active
    // slot is the `argument_index`-th schema slot; when it is variadic every
    // token after the fixed prefix is a value of that slot, so the whole
    // preceding token set is consumed by it.
    let active_is_last = !input.ends_with(char::is_whitespace) && !tokens.is_empty();
    let variadic_active = schema
        .get(argument_index)
        .is_some_and(|argument| argument.variadic);
    let preceding_count = if variadic_active {
        tokens.len().saturating_sub(usize::from(active_is_last))
    } else {
        argument_index.min(tokens.len())
    };
    let mut parsed: Vec<ParsedArgument> = Vec::new();
    for (index, token) in tokens.iter().take(preceding_count).enumerate() {
        let Some(argument) = schema
            .get(index)
            .or_else(|| schema.last().filter(|argument| argument.variadic))
        else {
            break;
        };
        let Ok(value) = parse_value(index, argument, token) else {
            continue;
        };
        if argument.variadic
            && parsed
                .last()
                .is_some_and(|previous| previous.name == argument.name)
        {
            let previous = parsed.last_mut().expect("checked above");
            let mut values = previous.values.to_vec();
            values.push(value);
            previous.values = values.into();
        } else {
            parsed.push(ParsedArgument {
                name: Arc::clone(&argument.name),
                values: Arc::from([value]),
                variadic: argument.variadic,
            });
        }
    }
    parsed.into()
}

pub fn parse_positional(
    input: &str,
    schema: Arc<[PositionalArgument]>,
) -> Result<ParsedArguments, ArgumentParseError> {
    let tokens = lex_strict(input)?;
    let mut arguments = Vec::with_capacity(schema.len());
    let mut token_index = 0;
    for (index, argument) in schema.iter().enumerate() {
        let count = if argument.variadic {
            tokens.len().saturating_sub(token_index)
        } else {
            usize::from(token_index < tokens.len())
        };
        if count == 0 && !argument.optional {
            return Err(ArgumentParseError::Count {
                expected: CommandArguments::Positional(Arc::clone(&schema)).arity(),
                actual: tokens.len(),
            });
        }
        if !argument.variadic && count == 1 {
            arguments.push(ParsedArgument {
                name: Arc::clone(&argument.name),
                values: Arc::from([parse_value(index, argument, &tokens[token_index])?]),
                variadic: false,
            });
            token_index += 1;
        } else if argument.variadic {
            let values = tokens[token_index..]
                .iter()
                .map(|token| parse_value(index, argument, token))
                .collect::<Result<Vec<_>, _>>()?;
            arguments.push(ParsedArgument {
                name: Arc::clone(&argument.name),
                values: values.into(),
                variadic: true,
            });
            token_index = tokens.len();
        } else {
            let values = argument
                .kind
                .default_value()
                .map(|value| Arc::from([ArgumentValue::Enum(Arc::from(value))]))
                .unwrap_or_else(|| Arc::from([]));
            arguments.push(ParsedArgument {
                name: Arc::clone(&argument.name),
                values,
                variadic: false,
            });
        }
        if argument.variadic {
            break;
        }
    }
    if token_index != tokens.len() {
        return Err(ArgumentParseError::Count {
            expected: CommandArguments::Positional(Arc::clone(&schema)).arity(),
            actual: tokens.len(),
        });
    }
    Ok(ParsedArguments::new(tokens, arguments))
}

fn parse_value(
    index: usize,
    argument: &PositionalArgument,
    token: &ParsedToken,
) -> Result<ArgumentValue, ArgumentParseError> {
    let value = token.value.as_ref();
    let result = match &argument.kind {
        ArgumentKind::String => Ok(ArgumentValue::String(Arc::clone(&token.value))),
        ArgumentKind::Integer => value
            .parse::<i64>()
            .ok()
            .filter(|number| (-MAX_EXACT_INTEGER..=MAX_EXACT_INTEGER).contains(number))
            .map(ArgumentValue::Integer)
            .ok_or_else(|| {
                Arc::from("must be a signed decimal in the exact portable integer range")
            }),
        ArgumentKind::Enum(choices) => choices
            .iter()
            .any(|choice| choice.as_ref() == value)
            .then(|| ArgumentValue::Enum(Arc::clone(&token.value)))
            .ok_or_else(|| Arc::from("is not a valid choice")),
        ArgumentKind::EnumWithDefault { choices, .. } => choices
            .iter()
            .any(|choice| choice.as_ref() == value)
            .then(|| ArgumentValue::Enum(Arc::clone(&token.value)))
            .ok_or_else(|| Arc::from("is not a valid choice")),
        ArgumentKind::File => path_value(value, false),
        ArgumentKind::Directory => path_value(value, true),
    };
    result.map_err(|message| ArgumentParseError::Invalid {
        index,
        name: Arc::clone(&argument.name),
        expected: Arc::from(argument.kind.type_name()),
        message,
        span: token.range.clone(),
    })
}

fn path_value(value: &str, directory: bool) -> Result<ArgumentValue, Arc<str>> {
    if value.is_empty() || value.as_bytes().contains(&0) {
        return Err(Arc::from("must be a nonempty NUL-free path spelling"));
    }
    let path = PathBuf::from(value);
    Ok(if directory {
        ArgumentValue::Directory(path)
    } else {
        ArgumentValue::File(path)
    })
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PathResolutionError {
    #[error("path spelling is empty or contains NUL")]
    InvalidSpelling,
    #[error("home directory is unavailable for '~' path")]
    HomeUnavailable,
    #[error("unsupported home-directory spelling: {0}")]
    UnsupportedHomeUser(Arc<str>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteStyle {
    None,
    Single,
    Double,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEdit {
    pub range: std::ops::Range<usize>,
    pub text: Arc<str>,
    pub cursor: usize,
}

#[allow(dead_code)]
pub fn encode_completion_value(
    value: &str,
    range: std::ops::Range<usize>,
    quote_style: QuoteStyle,
) -> CompletionEdit {
    let needs_quotes =
        value.is_empty() || value.chars().any(char::is_whitespace) || value.contains(['\'', '"']);
    let style = if quote_style != QuoteStyle::None
        && !value.contains(match quote_style {
            QuoteStyle::Single => '\'',
            QuoteStyle::Double => '"',
            QuoteStyle::None => '\0',
        }) {
        quote_style
    } else if needs_quotes {
        QuoteStyle::Double
    } else {
        QuoteStyle::None
    };
    let text = match style {
        QuoteStyle::None => value.to_owned(),
        QuoteStyle::Single => format!("'{value}'"),
        QuoteStyle::Double => {
            format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
        }
    };
    let cursor = range.start + text.len();
    CompletionEdit {
        range,
        text: Arc::from(text),
        cursor,
    }
}

pub fn resolve_path(
    cwd: &Path,
    home: Option<&Path>,
    spelling: &str,
) -> Result<PathBuf, PathResolutionError> {
    if spelling.is_empty() || spelling.as_bytes().contains(&0) {
        return Err(PathResolutionError::InvalidSpelling);
    }
    if spelling == "~" || spelling.starts_with("~/") || spelling.starts_with("~\\") {
        let home = home.ok_or(PathResolutionError::HomeUnavailable)?;
        return Ok(if spelling == "~" {
            home.to_path_buf()
        } else {
            home.join(&spelling[2..])
        });
    }
    if spelling.starts_with('~') {
        return Err(PathResolutionError::UnsupportedHomeUser(Arc::from(
            spelling,
        )));
    }
    let path = Path::new(spelling);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_decodes_quotes_and_fragments() {
        let tokens = lex_strict(r#"one "two three"'four'"#).unwrap();
        assert_eq!(tokens[0].value.as_ref(), "one");
    }

    #[test]
    fn integer_range_is_checked() {
        let schema: Arc<[PositionalArgument]> =
            Arc::from([PositionalArgument::required("n", ArgumentKind::Integer)]);
        assert!(parse_positional("9007199254740991", schema.clone()).is_ok());
        assert!(parse_positional("9007199254740992", schema).is_err());
    }

    #[test]
    fn quoted_input_parses_multiple_arguments() {
        let schema: Arc<[PositionalArgument]> = Arc::from([
            PositionalArgument::required("message", ArgumentKind::String),
            PositionalArgument::required("suffix", ArgumentKind::String),
        ]);

        let parsed = parse_positional(r#""hello world" tail"#, schema).unwrap();

        assert_eq!(parsed.tokens().len(), 2);
        assert_eq!(
            parsed.get("message"),
            Some(&ArgumentValue::String(Arc::from("hello world")))
        );
        assert_eq!(
            parsed.get("suffix"),
            Some(&ArgumentValue::String(Arc::from("tail")))
        );
    }

    #[test]
    fn typed_values_parse_enum_integer_and_paths() {
        let schema: Arc<[PositionalArgument]> = Arc::from([
            PositionalArgument::required(
                "mode",
                ArgumentKind::Enum(Arc::from([Arc::from("fast"), Arc::from("safe")])),
            ),
            PositionalArgument::required("count", ArgumentKind::Integer),
            PositionalArgument::required("file", ArgumentKind::File),
            PositionalArgument::required("directory", ArgumentKind::Directory),
        ]);

        let parsed = parse_positional("fast 42 src/main.rs target", schema).unwrap();

        assert_eq!(
            parsed.get("mode"),
            Some(&ArgumentValue::Enum(Arc::from("fast")))
        );
        assert_eq!(parsed.get("count"), Some(&ArgumentValue::Integer(42)));
        assert_eq!(
            parsed.get("file"),
            Some(&ArgumentValue::File(PathBuf::from("src/main.rs")))
        );
        assert_eq!(
            parsed.get("directory"),
            Some(&ArgumentValue::Directory(PathBuf::from("target")))
        );
    }

    #[test]
    fn encoded_completion_values_roundtrip_through_lexer() {
        assert_eq!(
            resolve_path(Path::new("/cwd"), Some(Path::new("/home")), "~").unwrap(),
            PathBuf::from("/home")
        );
        for value in ["plain", "two words", r#"a\\b\"c"#] {
            let edit = encode_completion_value(value, 3..3, QuoteStyle::None);
            let tokens = lex_strict(&edit.text).unwrap();

            assert_eq!(tokens.len(), 1);
            assert_eq!(tokens[0].value.as_ref(), value);
            assert_eq!(edit.cursor, 3 + edit.text.len());
        }
    }

    #[test]
    fn variadic_completion_prefix_includes_earlier_variadic_values() {
        let mut schema: Vec<PositionalArgument> = vec![
            PositionalArgument::required(
                "mode",
                ArgumentKind::Enum(Arc::from([Arc::from("fast"), Arc::from("safe")])),
            ),
            PositionalArgument::required("paths", ArgumentKind::Directory)
                .with_completion(CompletionPolicy::Default),
        ];
        schema[1].variadic = true;

        let parsed = parse_completion_prefix_arguments("fast one ", &schema, 1);
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0].name.as_ref(),
            "mode",
            "fixed slot must be exposed as a value"
        );
        assert_eq!(
            parsed[1].name.as_ref(),
            "paths",
            "the second path value must be exposed, not dropped"
        );
        assert_eq!(parsed[1].values.len(), 1);
        assert!(
            matches!(parsed[1].values[0], ArgumentValue::Directory(ref path) if path.as_os_str() == "one")
        );
    }
}
