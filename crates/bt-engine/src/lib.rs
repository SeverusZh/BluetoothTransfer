//! 传输引擎：助手协议状态机、OBEX 推送、顺序队列、断点续传、journal、打包与接收服务。
//!
//! 本层通过 bt-transport 的 trait 访问蓝牙，平台差异不进入本 crate 的公共接口。

pub mod cancel;
pub mod client;
pub mod detect;
pub mod journal;
pub mod md5;
pub mod obex;
pub mod opp;
pub mod package;
pub mod protocol;
pub mod queue;
pub mod receive;
pub mod resume;
pub mod send;
pub mod server;
pub mod sink;

pub use cancel::CancelFlag;
pub use client::{send_bytes, send_file, send_source, ChunkSource, SendOutcome};
pub use detect::{PresenceTracker, ProbeOutcome, ProbeResult, ProbeStrategy};
pub use journal::{
    ArtifactCleaner, BeginResult, ImportReport, Journal, JournalEntry, JournalKind, JournalState, ProcessLiveness,
    RecoveryPlan, MAX_ATTEMPTS, MAX_ENTRIES, MAX_ENTRY_BYTES, RENEW, TTL,
};
pub use obex::{auth_digest, ObexClient, ObexHeader, ObexPacket, ObexPushResult};
pub use opp::{is_retryable, mime_for_name, push_file, push_file_with_retry, OppOptions, OppRetryPolicy};
pub use package::{package_file, package_folder, plan_folder_package, PackagePlan, PackedSource, PendingPackageStore};
pub use protocol::{
    Ack, Cancel, DataSlice, Done, ErrorCode, ErrorMessage, Frame, FrameKind, Hello, Offer, OfferStatus, DATA_OVERHEAD,
    HEADER_LEN, MAX_CHUNK_SIZE, MAX_FRAME_SIZE, MAX_PAYLOAD,
};
pub use queue::{QueueJob, QueueManager, SpeedTracker};
pub use receive::{
    ReceiveAskNotice, ReceiveDoneNotice, ReceiveHooks, ReceiveProgressNotice, ReceiveService, ReceiveSettings,
    ReceiveSnapshot,
};
pub use resume::{ResumeSidecar, SIDECAR_LEN, SIDECAR_LIMIT};
pub use send::{OppSettings, SendEngine, SendEvent, SendSettings, SendSink};
pub use server::{send_busy, serve_connection, ServerEvent, ServerHooks, ServerOutcome};
pub use sink::{clean_stale_dir, FileSink, STALE_AFTER};
