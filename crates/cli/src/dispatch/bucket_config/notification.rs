//! `Data/S3/MyNotificationConfiguration.cs`, `MyTopicConfiguration.cs`, `MyQueueConfiguration.cs`,
//! `MyLambdaFunctionConfiguration.cs`: 알림 설정 입력과 SDK 형식 변환. `Filter`는 SDK의 `Filter` 모델을 그대로 읽는다.
//!
//! 원본 동작 그대로 둔 것
//! - 목록 요소가 `null`이면 `NullReferenceException`.
//! - 이벤트 문자열이 `null`이면 `new EventType(null)`이 `ArgumentNullException`을 던진다.
//! - 필터 규칙의 이름·값이 없으면 빈 문자열로 보낸다. 규칙 요소가 `null`이면 건너뛴다.
//! - `ExpectedBucketOwner`는 읽기만 하고 쓰지 않는다.

use aws_sdk_s3::types::{
    Event, FilterRule, FilterRuleName, LambdaFunctionConfiguration,
    NotificationConfigurationFilter, QueueConfiguration, S3KeyFilter, TopicConfiguration,
};
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};
use testrust_s3::UNSET;

use super::jsonutil::{argument_null_error, read_list};
use super::{CommandError, built, null_reference};

/// SDK `FilterRule`
#[derive(Debug, Default, Clone)]
struct FilterRuleInput {
    name: Option<String>,
    value: Option<String>,
}

impl FromJson for FilterRuleInput {
    fn type_name() -> String {
        "Amazon.S3.Model.FilterRule".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Name" => o.name = d.read_nullable()?,
                "Value" => o.value = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// SDK `S3KeyFilter`
#[derive(Debug, Default, Clone)]
struct S3KeyFilterInput {
    filter_rules: Option<Vec<Option<FilterRuleInput>>>,
}

impl FromJson for S3KeyFilterInput {
    fn type_name() -> String {
        "Amazon.S3.Model.S3KeyFilter".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "FilterRules" => o.filter_rules = read_list(d)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// SDK `Filter`
#[derive(Debug, Default, Clone)]
struct FilterInput {
    s3_key_filter: Option<S3KeyFilterInput>,
}

impl FromJson for FilterInput {
    fn type_name() -> String {
        "Amazon.S3.Model.Filter".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "S3KeyFilter" => o.s3_key_filter = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FilterInput {
    fn to_sdk(&self) -> NotificationConfigurationFilter {
        let key = self.s3_key_filter.as_ref().map(|key_filter| {
            let rules = key_filter.filter_rules.as_ref().map(|rules| {
                rules
                    .iter()
                    .flatten()
                    .map(|rule| {
                        FilterRule::builder()
                            .name(FilterRuleName::from(rule.name.as_deref().unwrap_or("")))
                            .value(rule.value.as_deref().unwrap_or(""))
                            .build()
                    })
                    .collect()
            });
            S3KeyFilter::builder().set_filter_rules(rules).build()
        });
        NotificationConfigurationFilter::builder()
            .set_key(key)
            .build()
    }
}

/// 이벤트 문자열 → `new EventType(item)`.
fn events_of(events: &Option<Vec<Option<String>>>) -> Result<Vec<Event>, CommandError> {
    events
        .iter()
        .flatten()
        .map(|event| match event {
            Some(event) => Ok(Event::from(event.as_str())),
            None => Err(argument_null_error()),
        })
        .collect()
}

/// 알림 한 항목(`MyTopicConfiguration` 등)의 공통 모양. 대상 이름 속성만 다르다.
#[derive(Debug, Default, Clone)]
struct Target {
    id: Option<String>,
    /// `Topic`, `Queue`, `FunctionArn`
    arn: Option<String>,
    events: Option<Vec<Option<String>>>,
    filter: Option<FilterInput>,
}

impl Target {
    fn read(
        d: &mut Deserializer<'_>,
        tok: Token,
        type_name: &str,
        arn_property: &str,
    ) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, type_name, Self::default(), |d, o, name| {
            if name == arn_property {
                o.arn = d.read_nullable()?;
                return Ok(true);
            }
            match name {
                "Id" => o.id = d.read_nullable()?,
                "Events" => o.events = read_list(d)?,
                "Filter" => o.filter = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }

    fn arn(&self) -> String {
        self.arn.clone().unwrap_or_else(|| UNSET.to_string())
    }
}

macro_rules! target_class {
    ($name:ident, $dotnet:literal, $arn_property:literal) => {
        #[derive(Debug, Default, Clone)]
        pub(super) struct $name(Target);

        impl FromJson for $name {
            fn type_name() -> String {
                $dotnet.into()
            }

            fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
                Ok(Target::read(d, tok, $dotnet, $arn_property)?.map(Self))
            }
        }
    };
}

target_class!(
    MyTopicConfiguration,
    "TestCore.Data.S3.MyTopicConfiguration",
    "Topic"
);
target_class!(
    MyQueueConfiguration,
    "TestCore.Data.S3.MyQueueConfiguration",
    "Queue"
);
target_class!(
    MyLambdaFunctionConfiguration,
    "TestCore.Data.S3.MyLambdaFunctionConfiguration",
    "FunctionArn"
);

impl MyTopicConfiguration {
    fn to_sdk(&self) -> Result<TopicConfiguration, CommandError> {
        let t = &self.0;
        Ok(built(
            TopicConfiguration::builder()
                .set_id(t.id.clone())
                .topic_arn(t.arn())
                .set_events(Some(events_of(&t.events)?))
                .set_filter(t.filter.as_ref().map(FilterInput::to_sdk))
                .build(),
        ))
    }
}

impl MyQueueConfiguration {
    fn to_sdk(&self) -> Result<QueueConfiguration, CommandError> {
        let t = &self.0;
        Ok(built(
            QueueConfiguration::builder()
                .set_id(t.id.clone())
                .queue_arn(t.arn())
                .set_events(Some(events_of(&t.events)?))
                .set_filter(t.filter.as_ref().map(FilterInput::to_sdk))
                .build(),
        ))
    }
}

impl MyLambdaFunctionConfiguration {
    fn to_sdk(&self) -> Result<LambdaFunctionConfiguration, CommandError> {
        let t = &self.0;
        Ok(built(
            LambdaFunctionConfiguration::builder()
                .set_id(t.id.clone())
                .lambda_function_arn(t.arn())
                .set_events(Some(events_of(&t.events)?))
                .set_filter(t.filter.as_ref().map(FilterInput::to_sdk))
                .build(),
        ))
    }
}

/// 목록의 각 요소를 변환한다. 목록이 없으면 빈 목록, 요소가 `null`이면 `NullReferenceException`.
fn convert<T, U>(
    items: &Option<Vec<Option<T>>>,
    convert: impl Fn(&T) -> Result<U, CommandError>,
) -> Result<Vec<U>, CommandError> {
    items
        .iter()
        .flatten()
        .map(|item| convert(item.as_ref().ok_or_else(null_reference)?))
        .collect()
}

/// `MyNotificationConfiguration`
#[derive(Debug, Default, Clone)]
pub(super) struct MyNotificationConfiguration {
    pub topic_configurations: Option<Vec<Option<MyTopicConfiguration>>>,
    pub queue_configurations: Option<Vec<Option<MyQueueConfiguration>>>,
    pub lambda_function_configurations: Option<Vec<Option<MyLambdaFunctionConfiguration>>>,
    pub expected_bucket_owner: Option<String>,
}

impl FromJson for MyNotificationConfiguration {
    fn type_name() -> String {
        "TestCore.Data.S3.MyNotificationConfiguration".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "TopicConfigurations" => o.topic_configurations = read_list(d)?,
                "QueueConfigurations" => o.queue_configurations = read_list(d)?,
                "LambdaFunctionConfigurations" => o.lambda_function_configurations = read_list(d)?,
                "ExpectedBucketOwner" => o.expected_bucket_owner = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl MyNotificationConfiguration {
    /// `GetTopicConfigurations()`
    pub(super) fn topic_configurations(&self) -> Result<Vec<TopicConfiguration>, CommandError> {
        convert(&self.topic_configurations, MyTopicConfiguration::to_sdk)
    }

    /// `GetQueueConfigurations()`
    pub(super) fn queue_configurations(&self) -> Result<Vec<QueueConfiguration>, CommandError> {
        convert(&self.queue_configurations, MyQueueConfiguration::to_sdk)
    }

    /// `GetLambdaFunctionConfigurations()`
    pub(super) fn lambda_function_configurations(
        &self,
    ) -> Result<Vec<LambdaFunctionConfiguration>, CommandError> {
        convert(
            &self.lambda_function_configurations,
            MyLambdaFunctionConfiguration::to_sdk,
        )
    }
}
