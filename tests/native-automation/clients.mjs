import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { isAbsolute } from 'node:path';
import { chromium } from 'playwright-core';
import puppeteer from 'puppeteer-core';
import { Builder, By, until } from 'selenium-webdriver';
import chrome from 'selenium-webdriver/chrome.js';
import { loopbackPort } from './loopback.mjs';

const marker = 'NUMERA_RENDERER_FIXTURE_PASS';
const frameMarker = 'NUMERA_FRAME_FIXTURE_PASS';
const fixtureText = 'Numera مسیر 2026';
const completedChecks = ['navigation', 'unicode-input', 'click-and-wait', 'nested-frame', 'png-screenshot'];
const pngSignature = Buffer.from('89504e470d0a1a0a', 'hex');

function verifyScreenshot(bytes) {
  const image = Buffer.from(bytes);
  assert.ok(image.length > pngSignature.length && image.subarray(0, 8).equals(pngSignature),
    'Expected a browser screenshot in PNG format');
}

async function measureViewport(page) {
  return page.evaluate(() => ({
    width: window.innerWidth,
    height: window.innerHeight,
    screenWidth: window.screen.width,
    screenHeight: window.screen.height,
    deviceScaleFactor: window.devicePixelRatio,
    touchPoints: navigator.maxTouchPoints,
  }));
}

async function testResponsiveViewport(browser, fixture) {
  const url = new URL('responsive-viewport.html', fixture).href;
  const untouched = await browser.newPage();
  const emulated = await browser.newPage();
  try {
    await untouched.goto(url, { timeout: 15000 });
    const desktop = await measureViewport(untouched);
    const session = await emulated.createCDPSession();
    const results = [];
    for (const layout of [{ width: 390, height: 844, scale: 3 }, { width: 844, height: 390, scale: 2 }]) {
      const { width, height, scale } = layout;
      await session.send('Emulation.setDeviceMetricsOverride', {
        mobile: true, width, height, screenWidth: width, screenHeight: height, deviceScaleFactor: scale,
      });
      await session.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
      await emulated.goto(url, { timeout: 15000 });
      const expected = { width, height, screenWidth: width, screenHeight: height, deviceScaleFactor: scale, touchPoints: 5 };
      const observed = await measureViewport(emulated);
      assert.deepEqual(observed, expected);
      await emulated.reload({ timeout: 15000 });
      assert.deepEqual(await measureViewport(emulated), expected);
      assert.deepEqual(await measureViewport(untouched), desktop);
      results.push(observed);
    }
    return { result: 'PASS', layouts: results, reload: 'PASS', separate_target: 'PASS',
      scope: 'Chromium CDP viewport, screen, pixel ratio and touch on a controlled local document' };
  } finally {
    await emulated.close();
    await untouched.close();
  }
}

async function testPuppeteer(endpoint, fixture) {
  const browser = await puppeteer.connect({ browserURL: endpoint, protocolTimeout: 15000 });
  try {
    const page = await browser.newPage();
    await page.goto(fixture, { timeout: 15000 });
    assert.equal(await page.$eval('#result', element => element.textContent), marker);
    await page.type('#entry', fixtureText);
    await page.click('#apply');
    await page.waitForFunction(value => document.getElementById('applied').textContent === value,
      { timeout: 15000 }, fixtureText);
    const iframe = await page.waitForSelector('#details', { timeout: 15000 });
    const frame = await iframe.contentFrame();
    await frame.waitForSelector('#frame-result', { timeout: 15000 });
    assert.equal(await frame.$eval('#frame-result', element => element.textContent), frameMarker);
    verifyScreenshot(await page.screenshot({ type: 'png' }));
    await page.close();
    return await testResponsiveViewport(browser, fixture);
  } finally {
    await browser.disconnect();
  }
}

async function testPlaywright(endpoint, fixture) {
  const browser = await chromium.connectOverCDP(endpoint, { timeout: 15000 });
  try {
    const page = await browser.contexts()[0].newPage();
    await page.goto(fixture, { timeout: 15000 });
    assert.equal(await page.locator('#result').textContent({ timeout: 15000 }), marker);
    await page.locator('#entry').fill(fixtureText, { timeout: 15000 });
    await page.locator('#apply').click({ timeout: 15000 });
    await page.waitForFunction(value => document.getElementById('applied').textContent === value,
      fixtureText, { timeout: 15000 });
    assert.equal(await page.frameLocator('#details').locator('#frame-result').textContent({ timeout: 15000 }), frameMarker);
    verifyScreenshot(await page.screenshot({ type: 'png', timeout: 15000 }));
    await page.close();
  } finally {
    await browser.close();
  }
}

const [driverAddress, browserPath, browserProfile, fixture, reportPath] = process.argv.slice(2);
assert.ok(isAbsolute(browserPath) && isAbsolute(browserProfile) && isAbsolute(reportPath));
assert.equal(new URL(fixture).protocol, 'file:');
const endpoint = loopbackPort(driverAddress);
const { dependencies, engines } = JSON.parse(await readFile(new URL('./package.json', import.meta.url), 'utf8'));
assert.equal(process.versions.node, engines.node);

const options = new chrome.Options().setChromeBinaryPath(browserPath).addArguments(
  '--headless=new', '--no-first-run', '--no-default-browser-check', `--user-data-dir=${browserProfile}`,
);
const driver = await new Builder().disableEnvironmentOverrides().usingServer(endpoint)
  .forBrowser('chrome').setChromeOptions(options).build();
let browserVersion, responsiveViewport;
try {
  await driver.manage().setTimeouts({ pageLoad: 15000, script: 15000 });
  await driver.get(fixture);
  assert.equal(await driver.findElement(By.id('result')).getText(), marker);
  await driver.findElement(By.id('entry')).sendKeys(fixtureText);
  await driver.findElement(By.id('apply')).click();
  await driver.wait(until.elementTextIs(driver.findElement(By.id('applied')), fixtureText), 15000);
  await driver.switchTo().frame(driver.findElement(By.id('details')));
  try {
    assert.equal(await driver.findElement(By.id('frame-result')).getText(), frameMarker);
  } finally {
    await driver.switchTo().defaultContent();
  }
  verifyScreenshot(Buffer.from(await driver.takeScreenshot(), 'base64'));
  const capabilities = await driver.getCapabilities();
  browserVersion = capabilities.get('browserVersion');
  assert.match(browserVersion, /^\d+\.\d+\.\d+\.\d+$/);
  const browserEndpoint = loopbackPort(capabilities.get('goog:chromeOptions')?.debuggerAddress);
  responsiveViewport = await testPuppeteer(browserEndpoint, fixture);
  await testPlaywright(browserEndpoint, fixture);
} finally {
  await driver.quit();
}

const tests = {
  selenium: { version: dependencies['selenium-webdriver'], transport: 'WebDriver', checks: completedChecks },
  puppeteer: { version: dependencies['puppeteer-core'], transport: 'Chrome DevTools Protocol', checks: completedChecks },
  playwright: { version: dependencies['playwright-core'], transport: 'connectOverCDP', checks: completedChecks },
};
await writeFile(reportPath, JSON.stringify({ result: 'PASS', node: process.versions.node, browserVersion, tests, responsiveViewport,
  scope: 'Controlled headless navigation, Unicode input, frame, click and screenshot for each client' }, null, 2) + '\n',
{ encoding: 'utf8', flag: 'wx' });
