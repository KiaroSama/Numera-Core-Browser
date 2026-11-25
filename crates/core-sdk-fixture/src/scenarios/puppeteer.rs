use super::timeout;
use crate::{
    node::{Context, Js, Result},
    run::{FRAME_MARKER, INPUT, MARKER, Run, TIMEOUT, expect_text},
};
use serde_json::json;

fn text(ctx: Context, parent: Js, selector: &str) -> Result<Js> {
    ctx.call(
        parent,
        "$eval",
        &[
            ctx.string(selector)?,
            ctx.dom_function(Some("element"), "return element.textContent;")?,
        ],
    )
}

fn page(state: &Run, ctx: Context) -> Result<Js> {
    state.object(ctx, "puppeteer_page")
}

pub(super) fn execute(state: &mut Run) {
    state.stage(|state, ctx, _| {
        let module = state.object(ctx, "puppeteer-core")?;
        let puppeteer = ctx.get(module, "default")?;
        ctx.call(
            puppeteer,
            "connect",
            &[ctx.json(&json!({
                "browserURL":state.browser_endpoint,"protocolTimeout":TIMEOUT
            }))?],
        )
    });
    state.keep("puppeteer_browser");
    state.act("puppeteer_browser", "newPage");
    state.keep("puppeteer_page");
    state.stage(|state, ctx, _| {
        ctx.call(
            page(state, ctx)?,
            "goto",
            &[ctx.string(&state.args.fixture)?, timeout(ctx)?],
        )
    });
    state.stage(|state, ctx, _| text(ctx, page(state, ctx)?, "#result"));
    state.stage(|_, ctx, previous| expect_text(ctx, previous, MARKER));
    state.stage(|state, ctx, _| {
        ctx.call(
            page(state, ctx)?,
            "type",
            &[ctx.string("#entry")?, ctx.string(INPUT)?],
        )
    });
    state.stage(|state, ctx, _| ctx.call(page(state, ctx)?, "click", &[ctx.string("#apply")?]));
    state.stage(|state, ctx, _| {
        ctx.call(
            page(state, ctx)?,
            "waitForFunction",
            &[
                ctx.dom_function(
                    Some("value"),
                    "return document.getElementById('applied').textContent === value;",
                )?,
                timeout(ctx)?,
                ctx.string(INPUT)?,
            ],
        )
    });
    state.stage(|state, ctx, _| {
        ctx.call(
            page(state, ctx)?,
            "waitForSelector",
            &[ctx.string("#details")?, timeout(ctx)?],
        )
    });
    state.stage(|_, ctx, iframe| ctx.call(iframe, "contentFrame", &[]));
    state.keep("puppeteer_frame");
    state.stage(|state, ctx, _| {
        ctx.call(
            state.object(ctx, "puppeteer_frame")?,
            "waitForSelector",
            &[ctx.string("#frame-result")?, timeout(ctx)?],
        )
    });
    state.stage(|state, ctx, _| text(ctx, state.object(ctx, "puppeteer_frame")?, "#frame-result"));
    state.stage(|_, ctx, previous| expect_text(ctx, previous, FRAME_MARKER));
    state.stage(|state, ctx, _| {
        ctx.call(
            page(state, ctx)?,
            "screenshot",
            &[ctx.json(&json!({"type":"png"}))?],
        )
    });
    state.stage(|_, ctx, previous| {
        ctx.screenshot(previous)?;
        ctx.undefined()
    });
    state.act("puppeteer_page", "close");
    responsive(state);
    state.act("puppeteer_browser", "disconnect");
}

fn measure(state: &Run, ctx: Context, page: &str) -> Result<Js> {
    ctx.call(state.object(ctx, page)?, "evaluate", &[
        ctx.dom_function(None,
            "return {width:window.innerWidth,height:window.innerHeight,screenWidth:window.screen.width,screenHeight:window.screen.height,deviceScaleFactor:window.devicePixelRatio,touchPoints:navigator.maxTouchPoints};")?,
    ])
}

fn responsive(state: &mut Run) {
    state.act("puppeteer_browser", "newPage");
    state.keep("desktop");
    state.act("puppeteer_browser", "newPage");
    state.keep("emulated");
    state.stage(|state, ctx, _| {
        ctx.call(
            state.object(ctx, "desktop")?,
            "goto",
            &[ctx.string(&state.args.responsive)?, timeout(ctx)?],
        )
    });
    state.stage(|state, ctx, _| measure(state, ctx, "desktop"));
    state.stage(|state, ctx, previous| {
        state.desktop = ctx.read_json(previous)?;
        ctx.undefined()
    });
    state.act("emulated", "createCDPSession");
    state.keep("emulation");
    for (width, height, scale) in [(390, 844, 3), (844, 390, 2)] {
        state.stage(move |state, ctx, _| {
            ctx.call(
                state.object(ctx, "emulation")?,
                "send",
                &[
                    ctx.string("Emulation.setDeviceMetricsOverride")?,
                    ctx.json(
                        &json!({"mobile":true,"width":width,"height":height,"screenWidth":width,
                    "screenHeight":height,"deviceScaleFactor":scale}),
                    )?,
                ],
            )
        });
        state.stage(|state, ctx, _| {
            ctx.call(
                state.object(ctx, "emulation")?,
                "send",
                &[
                    ctx.string("Emulation.setTouchEmulationEnabled")?,
                    ctx.json(&json!({"enabled":true,"maxTouchPoints":5}))?,
                ],
            )
        });
        state.stage(|state, ctx, _| {
            ctx.call(
                state.object(ctx, "emulated")?,
                "goto",
                &[ctx.string(&state.args.responsive)?, timeout(ctx)?],
            )
        });
        let expected = json!({"width":width,"height":height,"screenWidth":width,
            "screenHeight":height,"deviceScaleFactor":scale,"touchPoints":5});
        let expected_after_reload = expected.clone();
        state.stage(|state, ctx, _| measure(state, ctx, "emulated"));
        state.stage(move |state, ctx, previous| {
            let observed = ctx.read_json(previous)?;
            if observed != expected {
                return Err("Puppeteer emulated viewport did not match the expected layout".into());
            }
            state.layouts.push(observed);
            ctx.undefined()
        });
        state.stage(|state, ctx, _| {
            ctx.call(state.object(ctx, "emulated")?, "reload", &[timeout(ctx)?])
        });
        state.stage(|state, ctx, _| measure(state, ctx, "emulated"));
        state.stage(move |_, ctx, previous| {
            if ctx.read_json(previous)? != expected_after_reload {
                return Err("Puppeteer emulated viewport changed after reload".into());
            }
            ctx.undefined()
        });
        state.stage(|state, ctx, _| measure(state, ctx, "desktop"));
        state.stage(|state, ctx, previous| {
            if ctx.read_json(previous)? != state.desktop {
                return Err("Puppeteer emulation affected the independent desktop target".into());
            }
            ctx.undefined()
        });
    }
    state.act("emulated", "close");
    state.act("desktop", "close");
}
