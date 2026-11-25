// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `form_fill_batch`: not built yet (tests first).

use serde_json::Value;

use crate::{Args, Automation, Result, failed};

impl Automation {
    pub(crate) fn form_fill_batch(&self, _args: &Args) -> Result<Value> {
        Err(failed("form_fill_batch is not built yet"))
    }
}
