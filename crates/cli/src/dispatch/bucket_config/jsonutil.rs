//! 입력 JSON(`JsonSerializer.Deserialize<T>(text)`, 기본 옵션) 읽기 도우미.
//!
//! SDK 모델을 그대로 읽는 경우 System.Text.Json의 읽기 규칙이 모델 모양에 따라 달라진다.
//! - `int?`·`bool?` 속성은 변환 오류에 `System.Nullable`1[...]` 형식 이름이 나온다.
//! - 상수 클래스(`ServerSideEncryptionMethod`, `PartitionDateSource`)는 `{ "Value": "..." }` 객체로 읽고
//!   생성자(`value`)에 넘긴다. `Value`가 없거나 `null`이어도 읽을 때는 오류가 아니고, 요청 XML을 만들 때
//!   `ArgumentNullException`이 난다([`ConstantValue::get`]).
//! - 생성자가 둘인 `S3Permission`은 객체를 읽으려 하면 `NotSupportedException`이다.

use testrust_common::json::{Deserializer, FromJson, JsonError, ReadOptions, Token, deserialize};

use crate::dispatch::CommandError;

/// `JsonError`에 JsonException이 아닌 예외를 실어 나르는 표지(예외 형식과 메시지를 구분한다).
const OTHER: char = '\u{1}';

/// JSON 읽기 도중 JsonException이 아닌 예외가 난 경우.
pub(super) fn other_exception(dotnet_type: &str, message: &str) -> JsonError {
    JsonError(format!("{OTHER}{dotnet_type}{OTHER}{message}"))
}

/// `new EventType(null)`이나 값이 `null`인 상수 클래스를 문자열로 바꿀 때 나는 `ArgumentNullException`.
pub(super) fn argument_null_error() -> CommandError {
    CommandError::new(
        "System.ArgumentNullException",
        "Value cannot be null. (Parameter 'key')",
    )
}

/// `JsonSerializer.Deserialize<T>(text)`. JSON `null`이면 `None`.
pub(super) fn parse<T: FromJson>(text: &str) -> Result<Option<T>, CommandError> {
    deserialize::<T>(text, ReadOptions::default()).map_err(|error| {
        match error
            .0
            .strip_prefix(OTHER)
            .and_then(|rest| rest.split_once(OTHER))
        {
            Some((dotnet_type, message)) => CommandError::new(dotnet_type, message),
            None => CommandError::new("System.Text.Json.JsonException", error.0),
        }
    })
}

/// 다음 값을 `List<T>`로 읽는다(`null`이면 `None`, 요소가 `null`이면 `None`).
pub(super) fn read_list<T: FromJson>(
    d: &mut Deserializer<'_>,
) -> Result<Option<Vec<Option<T>>>, JsonError> {
    let tok = d.read_token()?;
    d.read_list::<T>(tok)
}

/// `int?`
pub(super) struct NullableI32(pub i32);

impl FromJson for NullableI32 {
    fn type_name() -> String {
        "System.Nullable`1[System.Int32]".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        match tok {
            Token::Null => Ok(None),
            Token::Number(text) => text
                .parse::<i32>()
                .map(|value| Some(Self(value)))
                .map_err(|_| d.conversion_error(&Self::type_name())),
            _ => Err(d.conversion_error(&Self::type_name())),
        }
    }
}

/// `bool?`
pub(super) struct NullableBool(pub bool);

impl FromJson for NullableBool {
    fn type_name() -> String {
        "System.Nullable`1[System.Boolean]".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        match tok {
            Token::Null => Ok(None),
            Token::True => Ok(Some(Self(true))),
            Token::False => Ok(Some(Self(false))),
            _ => Err(d.conversion_error(&Self::type_name())),
        }
    }
}

/// 상수 클래스 객체(`{ "Value": "..." }`). `Value`가 없거나 `null`이면 값 없는 객체다.
#[derive(Debug, Clone, Default)]
pub(super) struct ConstantValue(Option<String>);

impl ConstantValue {
    /// 문자열 값. 값이 없는 객체는 요청 XML을 만들 때 `ArgumentNullException`이다.
    pub(super) fn get(&self) -> Result<&str, CommandError> {
        self.0.as_deref().ok_or_else(argument_null_error)
    }
}

/// 상수 클래스 객체를 읽는다.
fn read_constant(
    d: &mut Deserializer<'_>,
    tok: Token,
    type_name: &str,
) -> Result<Option<ConstantValue>, JsonError> {
    d.read_object(
        tok,
        type_name,
        ConstantValue::default(),
        |d, constant, name| {
            if name == "Value" {
                constant.0 = d.read_nullable::<String>()?;
                return Ok(true);
            }
            Ok(false)
        },
    )
}

macro_rules! constant_class {
    ($name:ident, $dotnet:literal) => {
        #[derive(Debug, Clone)]
        pub(super) struct $name(pub ConstantValue);

        impl FromJson for $name {
            fn type_name() -> String {
                $dotnet.into()
            }

            fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
                Ok(read_constant(d, tok, $dotnet)?.map(Self))
            }
        }
    };
}

constant_class!(
    ServerSideEncryptionMethod,
    "Amazon.S3.ServerSideEncryptionMethod"
);
constant_class!(PartitionDateSource, "Amazon.S3.PartitionDateSource");

/// `Amazon.S3.S3Permission`: 공개 생성자가 둘이라 객체를 읽을 수 없다(`null`만 가능).
pub(super) struct S3Permission;

impl FromJson for S3Permission {
    fn type_name() -> String {
        "Amazon.S3.S3Permission".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        match tok {
            Token::Null => Ok(None),
            Token::StartObject => {
                // 변환 오류와 같은 위치 정보(`Path: ...`)를 쓴다.
                let located = d.conversion_error(&Self::type_name()).0;
                let position = located
                    .split_once("Path:")
                    .map_or("", |(_, position)| position);
                Err(other_exception(
                    "System.NotSupportedException",
                    &format!(
                        "Deserialization of types without a parameterless constructor, a singular parameterized constructor, or a parameterized constructor annotated with 'JsonConstructorAttribute' is not supported. Type 'Amazon.S3.S3Permission'. Path:{position}"
                    ),
                ))
            }
            _ => Err(d.conversion_error(&Self::type_name())),
        }
    }
}
