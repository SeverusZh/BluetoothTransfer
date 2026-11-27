//! 传输引擎骨架：协议状态机、发送队列、断点续传、journal、打包与探测策略。
//!
//! 本层不直接接触操作系统蓝牙接口，平台差异由 bt-transport 抽象；
//! 本轮只沉淀协议常量、数据格式与队列骨架，真实调度在 T3 落地。

pub mod detect;
pub mod journal;
pub mod package;
pub mod protocol;
pub mod queue;
pub mod resume;

pub use detect::ProbeStrategy;
pub use journal::{JournalEntry, MAX_JOURNAL_ENTRY_BYTES};
pub use package::{package_folder, plan_folder_package, PackagePlan};
pub use protocol::{DataSlice, Frame, FrameKind, DATA_OVERHEAD, HEADER_LEN, MAX_FRAME_SIZE};
pub use queue::{QueueJob, QueueManager};
pub use resume::{ResumeSidecar, SIDECAR_LEN};
