use parking_lot::Mutex;
use std::sync::Arc;

use crate::stats::LiveStats;

pub type SharedStats = Arc<Mutex<LiveStats>>;
