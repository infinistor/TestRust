//! Local·Curl·ZeroMq·MultiSystem·UpDown 클라이언트, Portal·Jenkins·Mover
//!
//! 이식한 것: `KHttpClient`([`khttp`]), `PortalManager`([`portal`]), `CurlClient`([`curl`]),
//! `MoverClient`([`mover`]), `ZeroMqClient`([`zeromq`]), `UpDownClient`([`up_down`]),
//! `LocalClient`([`local`]), `MultiSystemClient`([`multi_system`]).
//!
//! 이식하지 않은 것: `Jenkins/JenkinsManager.cs`, `Util/Curl.cs`. TESTCore 안에서 어디서도 쓰지 않는다
//! (`JenkinsManager`·`Curl` 형식을 참조하는 코드가 없다). `JenkinsConfig`는 설정 파일 읽기용이라 `testrust-config`에 있다.

pub mod curl;
pub mod file_util;
mod http;
pub use testrust_common::json;
pub mod khttp;
pub mod local;
pub mod mover;
pub mod multi_system;
pub mod portal;
pub mod up_down;
pub mod zeromq;

pub use local::{LocalClient, LocalError};
pub use multi_system::{MultiSystemClient, MultiSystemError};
pub use up_down::{UpDownClient, UpDownError};
