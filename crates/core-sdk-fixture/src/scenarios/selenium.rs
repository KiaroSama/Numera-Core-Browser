use crate::{
    node::{Context, Js, Result, check_png},
    run::{FRAME_MARKER, INPUT, MARKER, Run, TIMEOUT, expect_text, loopback},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;

fn find(state: &Run, ctx: Context, id: &str) -> Result<Js> {
    let selenium = state.object(ctx, "selenium-webdriver")?;
    let by = ctx.get(selenium, "By")?;
    let locator = ctx.call(by, "id", &[ctx.string(id)?])?;
    ctx.call(state.object(ctx, "driver")?, "findElement", &[locator])
}

pub(super) fn execute(state: &mut Run) {
    state.stage(|state, ctx, _| {
        let chrome = state.object(ctx, "chrome")?;
        let opts = ctx.construct(ctx.get(chrome, "Options")?, &[])?;
        ctx.call(
            opts,
            "setChromeBinaryPath",
            &[ctx.string(&state.args.browser)?],
        )?;
        let profile = format!("--user-data-dir={}", state.args.profile);
        let args = [
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            &profile,
        ]
        .map(|arg| ctx.string(arg))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        ctx.call(opts, "addArguments", &args)?;
        let selenium = state.object(ctx, "selenium-webdriver")?;
        let builder = ctx.construct(ctx.get(selenium, "Builder")?, &[])?;
        ctx.call(builder, "disableEnvironmentOverrides", &[])?;
        ctx.call(builder, "usingServer", &[ctx.string(&state.args.driver)?])?;
        ctx.call(builder, "forBrowser", &[ctx.string("chrome")?])?;
        ctx.call(builder, "setChromeOptions", &[opts])?;
        ctx.call(builder, "build", &[])
    });
    state.keep("driver");
    state.stage(|state, ctx, _| {
        let manager = ctx.call(state.object(ctx, "driver")?, "manage", &[])?;
        ctx.call(
            manager,
            "setTimeouts",
            &[ctx.json(&json!({"pageLoad":TIMEOUT,"script":TIMEOUT}))?],
        )
    });
    state.stage(|state, ctx, _| {
        ctx.call(
            state.object(ctx, "driver")?,
            "get",
            &[ctx.string(&state.args.fixture)?],
        )
    });
    state.stage(|state, ctx, _| ctx.call(find(state, ctx, "result")?, "getText", &[]));
    state.stage(|_, ctx, previous| expect_text(ctx, previous, MARKER));
    state.stage(|state, ctx, _| {
        ctx.call(
            find(state, ctx, "entry")?,
            "sendKeys",
            &[ctx.string(INPUT)?],
        )
    });
    state.stage(|state, ctx, _| ctx.call(find(state, ctx, "apply")?, "click", &[]));
    state.stage(|state, ctx, _| {
        let selenium = state.object(ctx, "selenium-webdriver")?;
        let until = ctx.get(selenium, "until")?;
        let condition = ctx.call(
            until,
            "elementTextIs",
            &[find(state, ctx, "applied")?, ctx.string(INPUT)?],
        )?;
        ctx.call(
            state.object(ctx, "driver")?,
            "wait",
            &[condition, ctx.json(&json!(TIMEOUT))?],
        )
    });
    state.stage(|state, ctx, _| {
        let target = ctx.call(state.object(ctx, "driver")?, "switchTo", &[])?;
        ctx.call(target, "frame", &[find(state, ctx, "details")?])
    });
    state.stage(|state, ctx, _| ctx.call(find(state, ctx, "frame-result")?, "getText", &[]));
    state.stage(|_, ctx, previous| expect_text(ctx, previous, FRAME_MARKER));
    state.stage(|state, ctx, _| {
        let target = ctx.call(state.object(ctx, "driver")?, "switchTo", &[])?;
        ctx.call(target, "defaultContent", &[])
    });
    state.act("driver", "takeScreenshot");
    state.stage(|_, ctx, previous| {
        let bytes = STANDARD
            .decode(ctx.text(previous)?)
            .map_err(|error| error.to_string())?;
        if bytes.len() <= 8 {
            return Err("Expected a browser screenshot in PNG format".into());
        }
        check_png(&bytes)?;
        ctx.undefined()
    });
    state.act("driver", "getCapabilities");
    state.stage(|state, ctx, previous| {
        let version = ctx.call(previous, "get", &[ctx.string("browserVersion")?])?;
        let version = ctx.text(version)?;
        let parts: Vec<_> = version.split('.').collect();
        if parts.len() != 4
            || parts
                .iter()
                .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err("Browser version does not match Chromium version format".into());
        }
        state.browser_version = version;
        let chrome = ctx.call(previous, "get", &[ctx.string("goog:chromeOptions")?])?;
        state.browser_endpoint = loopback(&ctx.text(ctx.get(chrome, "debuggerAddress")?)?)?;
        ctx.undefined()
    });
}
