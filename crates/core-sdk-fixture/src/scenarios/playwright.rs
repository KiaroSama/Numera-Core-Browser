use super::timeout;
use crate::{
    node::{Context, Js, Result},
    run::{FRAME_MARKER, INPUT, MARKER, Run, TIMEOUT, expect_text},
};
use serde_json::json;

fn page(state: &Run, ctx: Context) -> Result<Js> {
    state.object(ctx, "playwright_page")
}

fn locator(ctx: Context, page: Js, selector: &str) -> Result<Js> {
    ctx.call(page, "locator", &[ctx.string(selector)?])
}

pub(super) fn execute(state: &mut Run) {
    state.stage(|state, ctx, _| {
        let module = state.object(ctx, "playwright-core")?;
        let chromium = ctx.get(module, "chromium")?;
        ctx.call(
            chromium,
            "connectOverCDP",
            &[ctx.string(&state.browser_endpoint)?, timeout(ctx)?],
        )
    });
    state.keep("playwright_browser");
    state.stage(|state, ctx, _| {
        let contexts = ctx.call(state.object(ctx, "playwright_browser")?, "contexts", &[])?;
        ctx.call(ctx.element(contexts, 0)?, "newPage", &[])
    });
    state.keep("playwright_page");
    state.stage(|state, ctx, _| {
        ctx.call(
            page(state, ctx)?,
            "goto",
            &[ctx.string(&state.args.fixture)?, timeout(ctx)?],
        )
    });
    state.stage(|state, ctx, _| {
        ctx.call(
            locator(ctx, page(state, ctx)?, "#result")?,
            "textContent",
            &[timeout(ctx)?],
        )
    });
    state.stage(|_, ctx, previous| expect_text(ctx, previous, MARKER));
    state.stage(|state, ctx, _| {
        ctx.call(
            locator(ctx, page(state, ctx)?, "#entry")?,
            "fill",
            &[ctx.string(INPUT)?, timeout(ctx)?],
        )
    });
    state.stage(|state, ctx, _| {
        ctx.call(
            locator(ctx, page(state, ctx)?, "#apply")?,
            "click",
            &[timeout(ctx)?],
        )
    });
    state.stage(|state, ctx, _| {
        ctx.call(
            page(state, ctx)?,
            "waitForFunction",
            &[
                ctx.dom_function(
                    Some("value"),
                    "return document.getElementById('applied').textContent === value;",
                )?,
                ctx.string(INPUT)?,
                timeout(ctx)?,
            ],
        )
    });
    state.stage(|state, ctx, _| {
        let frame = ctx.call(
            page(state, ctx)?,
            "frameLocator",
            &[ctx.string("#details")?],
        )?;
        ctx.call(
            locator(ctx, frame, "#frame-result")?,
            "textContent",
            &[timeout(ctx)?],
        )
    });
    state.stage(|_, ctx, previous| expect_text(ctx, previous, FRAME_MARKER));
    state.stage(|state, ctx, _| {
        ctx.call(
            page(state, ctx)?,
            "screenshot",
            &[ctx.json(&json!({"type":"png","timeout":TIMEOUT}))?],
        )
    });
    state.stage(|_, ctx, previous| {
        ctx.screenshot(previous)?;
        ctx.undefined()
    });
    state.act("playwright_page", "close");
    state.act("playwright_browser", "close");
}
