mod playwright;
mod puppeteer;
mod selenium;

use crate::{
    node::{Context, Js, Result},
    run::{Run, TIMEOUT, write_report},
};
use serde_json::json;

pub fn compose(state: &mut Run) {
    selenium::execute(state);
    puppeteer::execute(state);
    playwright::execute(state);
    state.act("driver", "quit");
    state.stage(|state, ctx, _| {
        write_report(state)?;
        ctx.undefined()
    });
}

fn timeout(ctx: Context) -> Result<Js> {
    ctx.json(&json!({"timeout": TIMEOUT}))
}
