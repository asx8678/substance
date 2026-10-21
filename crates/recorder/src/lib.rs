//! Driver host, the database, log, egress and email channels, normalization,
//! the record store and replay.
//!
//! Only the driver host exists so far (M3-T02); the rest of the crate's tasks add the channels
//! that the host's record parts are merged with.

pub mod driver;
