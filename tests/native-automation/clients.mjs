import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { isAbsolute } from 'node:path';
import { chromium } from 'playwright-core';
import puppeteer from 'puppeteer-core';
import { Builder, By } from 'selenium-webdriver';
import chrome from 'selenium-webdriver/chrome.js';

const marker = 'NUMERA_RENDERER_FIXTURE_PASS';
const pngSignature = Buffer.from('89504e470d0a1a0a', 'hex');

function verifyScreenshot(bytes) {
  const image = Buffer.from(bytes);
  assert.ok(image.length > pngSignature.length && image.subarray(0, 8).equals(pngSignature),
    'Expected a browser screenshot in PNG format');
}

function loopbackPort(address) {
  const match = /^127\.0\.0\.1:([1-9][0-9]{0,4})$/.exec(address);
  assert.ok(match && Number(match[1]) <= 65535, 'Expected a local Chromium automation endpoint');
  return `http://${match[0]}`;
}

async function testPuppeteer(endpoint, fixture) {
  const browser = await puppeteer.connect({ browserURL: endpoint, protocolTimeout: 15000 });
  try {
    const page = await browser.newPage();
    await page.goto(fixture, { timeout: 15000 });
    assert.equal(await page.$eval('#result', element => element.textContent), marker);
    verifyScreenshot(await page.screenshot({ type: 'png' }));
    await page.close();
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
try {
  await driver.manage().setTimeouts({ pageLoad: 15000, script: 15000 });
  await driver.get(fixture);
  assert.equal(await driver.findElement(By.id('result')).getText(), marker);
  verifyScreenshot(Buffer.from(await driver.takeScreenshot(), 'base64'));
  const capabilities = await driver.getCapabilities();
  const browserEndpoint = loopbackPort(capabilities.get('goog:chromeOptions')?.debuggerAddress);
  await testPuppeteer(browserEndpoint, fixture);
  await testPlaywright(browserEndpoint, fixture);
} finally {
  await driver.quit();
}

const tests = {
  selenium: { version: dependencies['selenium-webdriver'], transport: 'WebDriver' },
  puppeteer: { version: dependencies['puppeteer-core'], transport: 'Chrome DevTools Protocol' },
  playwright: { version: dependencies['playwright-core'], transport: 'connectOverCDP' },
};
await writeFile(reportPath, JSON.stringify({ result: 'PASS', node: process.versions.node, tests,
  scope: 'Controlled local navigation, DOM result and screenshot for each client' }, null, 2) + '\n',
{ encoding: 'utf8', flag: 'wx' });
