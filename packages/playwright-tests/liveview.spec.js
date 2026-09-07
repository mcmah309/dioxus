// @ts-check
const { test, expect } = require("@playwright/test");

test("button click", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");

  // Expect the page to contain the counter text.
  const main = page.locator("#main");
  await expect(main).toContainText("hello axum! 0");

  // Click the increment button.
  await page.getByRole("button", { name: "Increment" }).click();

  // Expect the page to contain the updated counter text.
  await expect(main).toContainText("hello axum! 1");
});

test("svg", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");

  // Expect the page to contain the svg.
  const svg = page.locator("svg");

  // Expect the svg to contain the circle.
  const circle = svg.locator("circle");
  await expect(circle).toHaveAttribute("cx", "50");
  await expect(circle).toHaveAttribute("cy", "50");
  await expect(circle).toHaveAttribute("r", "40");
  await expect(circle).toHaveAttribute("stroke", "green");
  await expect(circle).toHaveAttribute("fill", "yellow");
});

test("raw attribute", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");

  // Expect the page to contain the div with the raw attribute.
  const div = page.locator("div.raw-attribute-div");
  await expect(div).toHaveAttribute("raw-attribute", "raw-attribute-value");
});

test("hidden attribute", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");

  // Expect the page to contain the div with the hidden attribute.
  const div = page.locator("div.hidden-attribute-div");
  await expect(div).toHaveAttribute("hidden", "true");
});

test("dangerous inner html", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");

  // Expect the page to contain the div with the dangerous inner html.
  const div = page.locator("div.dangerous-inner-html-div");
  await expect(div).toContainText("hello dangerous inner html");
});

test("input value", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");

  // Expect the page to contain the input with the value.
  const input = page.locator("#input-value");
  await expect(input).toHaveValue("hello input");
});

test("style", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");

  // Expect the page to contain the div with the style.
  const div = page.locator("div.style-div");
  await expect(div).toHaveText("colored text");
  await expect(div).toHaveCSS("color", "rgb(255, 0, 0)");
});

test("onmounted", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");

  // Expect the onmounted event to be called exactly once.
  const mountedDiv = page.locator("div.onmounted-div");
  await expect(mountedDiv).toHaveText("onmounted was called 1 times");
});

test("file picker opens the browser dialog", async ({ page }) => {
  /** @type {string[]} */
  const requests = [];
  page.on("request", (request) => {
    if (request.url().includes("__file_dialog")) requests.push(request.url());
  });
  await page.goto("http://127.0.0.1:3030");

  for (const picker of [page.locator("#file-picker"), page.locator('label[for="file-picker"]')]) {
    const chooser = page.waitForEvent("filechooser", { timeout: 3000 });
    await picker.click();
    await (await chooser).setFiles([]);
  }
  expect(requests).toEqual([]);
});

for (const id of ["file-picker", "form-file-picker"]) {
  test(`file picker uploads contents through input and change (${id})`, async ({ page }) => {
    /** @type {string[]} */
    const messages = [];
    page.on("websocket", (socket) => {
      socket.on("framesent", ({ payload }) => messages.push(payload.toString()));
    });
    await page.goto("http://127.0.0.1:3030");
    await expect(page.locator(".onmounted-div")).toHaveText("onmounted was called 1 times");
    messages.length = 0;
    const picker = page.locator(`#${id}`);
    await picker.setInputFiles({
      name: "greeting.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("hello"),
    });
    const modified = await picker.evaluate((input) => /** @type {HTMLInputElement} */ (input).files[0].lastModified);
    const expected = `greeting.txt|5|text/plain|${modified}|[104, 101, 108, 108, 111]`;
    await expect(page.locator(`#${id}-input`)).toHaveText(expected);
    await expect(page.locator(`#${id}-change`)).toHaveText(expected);
    await expect(page.locator(`#${id}-values`)).toHaveText(expected);
    await expect(page.locator(`#${id}-text`)).toHaveText("hello");
    await expect(page.locator(`#${id}-counts`)).toHaveText("1,1");
    const events = messages
      .filter((message) => message !== "__ping__")
      .map((message) => JSON.parse(message))
      .filter((message) => message.method === "user_event")
      .map((message) => message.params.name);
    expect(events).toEqual(["input", "change"]);

    if (id === "form-file-picker") {
      await expect(page.locator(`#${id}-description`)).toHaveText("upload description");
      await page.locator("#upload-form").dispatchEvent("submit");
      await expect(page.locator("#submitted-files")).toHaveText(expected);
    }

    await picker.setInputFiles([]);
    await expect(page.locator(`#${id}-counts`)).toHaveText("2,2");
    await expect(page.locator(`#${id}-input`)).toHaveText("");
    await expect(page.locator(`#${id}-change`)).toHaveText("");
    await expect(page.locator(`#${id}-values`)).toHaveText("");
  });

  test(`file picker preserves duplicate names, binary and empty files (${id})`, async ({ page }) => {
    await page.goto("http://127.0.0.1:3030");
    const picker = page.locator(`#${id}`);
    await picker.setInputFiles([
      { name: "same.bin", mimeType: "application/octet-stream", buffer: Buffer.from([0, 255, 128]) },
      { name: "same.bin", mimeType: "application/octet-stream", buffer: Buffer.from([42]) },
      { name: "empty.txt", mimeType: "text/plain", buffer: Buffer.alloc(0) },
    ]);
    const modified = await picker.evaluate((input) => Array.from(/** @type {HTMLInputElement} */ (input).files, (file) => file.lastModified));
    const expected = [
      `same.bin|3|application/octet-stream|${modified[0]}|[0, 255, 128]`,
      `same.bin|1|application/octet-stream|${modified[1]}|[42]`,
      `empty.txt|0|text/plain|${modified[2]}|[]`,
    ].join("\n");
    await expect(page.locator(`#${id}-input`)).toHaveText(expected);
    await expect(page.locator(`#${id}-change`)).toHaveText(expected);
    await expect(page.locator(`#${id}-values`)).toHaveText(expected);
    await expect(page.locator(`#${id}-counts`)).toHaveText("1,1");
  });
}

test("text edits preserve order without rereading selected files", async ({ page }) => {
  /** @type {any[]} */
  const events = [];
  page.on("websocket", (socket) => {
    socket.on("framesent", ({ payload }) => {
      const text = payload.toString();
      if (!text.startsWith("{")) return;
      const message = JSON.parse(text);
      if (message.method === "user_event") events.push(message.params);
    });
  });
  await page.goto("http://127.0.0.1:3030");
  await expect(page.locator(".onmounted-div")).toHaveText("onmounted was called 1 times");
  events.length = 0;

  // Delay the first file read while changing the selection and typing.
  await page.evaluate(() => {
    const original = File.prototype.arrayBuffer;
    let reads = 0;
    let release;
    const gate = new Promise((resolve) => { release = resolve; });
    Object.assign(window, {
      fileReadCount: () => reads,
      releaseFileRead: () => release(),
    });
    File.prototype.arrayBuffer = async function () {
      if (++reads === 1) await gate;
      return original.call(this);
    };

    const picker = /** @type {HTMLInputElement} */ (document.querySelector("#form-file-picker"));
    const description = /** @type {HTMLInputElement} */ (document.querySelector('input[name="description"]'));
    for (const value of ["a", "ab", "abc"]) {
      if (value !== "abc") {
        const files = new DataTransfer();
        files.items.add(new File([value], `${value}.txt`, { type: "text/plain" }));
        picker.files = files.files;
        picker.dispatchEvent(new Event("change", { bubbles: true }));
      }
      description.value = value;
      description.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });
  await page.evaluate(() => /** @type {any} */ (window).releaseFileRead());

  await expect(page.locator("#description-values")).toHaveText("a,ab,abc");
  await expect(page.locator("#form-file-picker-counts")).toHaveText("0,2");
  expect(events.map(({ name }) => name)).toEqual(["change", "input", "change", "input", "input"]);
  const changes = events.filter(({ name }) => name === "change");
  expect(changes.map(({ data }) => data.values.find(({ key }) => key === "uploads").file.contents))
    .toEqual([[97], [97, 98]]);
  const inputs = events.filter(({ name }) => name === "input");
  expect(inputs.every(({ data }) => data.values.every(({ file }) => !file?.contents))).toBe(true);
  expect(await page.evaluate(() => /** @type {any} */ (window).fileReadCount())).toBe(2);
});

test("empty file fields remain present in input and submit events", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");
  await page.locator('input[name="description"]').fill("empty selection");
  await expect(page.locator("#upload-selected")).toHaveText("Some(File(None))");

  await page.locator("#form-file-picker").setInputFiles({
    name: "hello.txt", mimeType: "text/plain", buffer: Buffer.from("hello"),
  });
  await expect(page.locator("#form-file-picker-counts")).toHaveText("1,1");
  await page.locator("#upload-form").dispatchEvent("submit");
  await expect(page.locator("#upload-selected")).toContainText('name: "hello.txt"');

  await page.locator("#form-file-picker").setInputFiles([]);
  await expect(page.locator("#form-file-picker-counts")).toHaveText("2,2");
  await page.locator("#upload-form").dispatchEvent("submit");
  await expect(page.locator("#upload-selected")).toHaveText("Some(File(None))");
  await expect(page.locator("#submitted-files")).toHaveText("");
});

test("a failed file read does not block subsequent events", async ({ page }) => {
  /** @type {string[]} */
  const errors = [];
  /** @type {Error[]} */
  const unhandled = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => unhandled.push(error));
  await page.goto("http://127.0.0.1:3030");
  await page.evaluate(() => {
    File.prototype.arrayBuffer = () => Promise.reject(new DOMException("File unavailable", "NotReadableError"));
  });
  await page.locator("#form-file-picker").setInputFiles({
    name: "unreadable.txt", mimeType: "text/plain", buffer: Buffer.from("hello"),
  });
  await page.locator('input[name="description"]').fill("still responsive");
  await expect(page.locator("#description-values")).toHaveText("still responsive");
  expect(errors.filter((message) => message.includes("Failed to send LiveView event"))).toHaveLength(2);
  expect(unhandled).toEqual([]);
});
