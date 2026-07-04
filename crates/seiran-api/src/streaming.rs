//! ストリーミングハブ（#37）。実体は seiran-common に移設し、mono バイナリで
//! api ロールと federation ロールが同一インスタンスを共有できるようにした。

pub use seiran_common::streaming::{StreamEvent, StreamHub};
