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
      socket.on("framesent", ({ payload }) => {
        const text = payload.toString();
        if (text.startsWith("{")) messages.push(text);
      });
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
      .filter((message) => message.method === "user_event" || message.method === "file_upload")
      .map((message) => message.method === "file_upload" ? message.params.event.name : message.params.name);
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

test("multiple files, including a large file, upload over HTTP", async ({ page }) => {
  let closed = false;
  let uploadMetadata;
  const uploadRequests = [];
  page.on("websocket", (socket) => {
    socket.on("framesent", ({ payload }) => {
      expect(typeof payload).toBe("string");
      if (payload.startsWith("{")) {
        const message = JSON.parse(payload);
        if (message.method === "file_upload") uploadMetadata = message.params;
      }
    });
    socket.on("close", () => { closed = true; });
  });
  page.on("request", (request) => {
    if (request.url().includes("/ws/upload/")) uploadRequests.push(request);
  });
  await page.goto("http://127.0.0.1:3030");
  await expect(page.locator(".onmounted-div")).toHaveText("onmounted was called 1 times");

  const largeSize = 17 * 1024 * 1024;
  const secondSize = 1024 * 1024;
  await page.locator("#large-file-picker").evaluate((input, { largeSize, secondSize }) => {
    const largeBytes = new Uint8Array(largeSize);
    largeBytes.fill(255);
    const secondBytes = new Uint8Array(secondSize);
    secondBytes.fill(42);
    const files = new DataTransfer();
    files.items.add(new File([largeBytes], "large.bin", { type: "application/octet-stream" }));
    files.items.add(new File([secondBytes], "second.bin", { type: "application/octet-stream" }));
    /** @type {HTMLInputElement} */ (input).files = files.files;
    input.dispatchEvent(new Event("change", { bubbles: true }));
  }, { largeSize, secondSize });

  await expect(page.locator("#large-upload")).toHaveText([
    `large.bin|${largeSize}|255|255`,
    `second.bin|${secondSize}|42|42`,
  ].join("\n"));
  expect(closed).toBe(false);
  expect(uploadMetadata.size).toBe(largeSize + secondSize);
  expect(uploadRequests).toHaveLength(2);
  for (const [request, name, size] of [
    [uploadRequests[0], "large.bin", largeSize],
    [uploadRequests[1], "second.bin", secondSize],
  ]) {
    expect(request.method()).toBe("PUT");
    expect(new URL(request.url()).pathname).toMatch(/^\/ws\/upload\/[0-9a-f-]+$/);
    const uploadHeaders = await request.allHeaders();
    expect(uploadHeaders["content-type"]).toBe("application/octet-stream");
    expect(uploadHeaders["content-length"]).toBe(size.toString());
    expect(uploadHeaders["x-content-size"]).toBe(size.toString());
    expect(uploadHeaders["content-disposition"]).toBe(
      `attachment; filename="${name}"`
    );
    expect(uploadHeaders["x-request-client"]).toBe("dioxus");
  }
});

test("file uploads work when running an existing VirtualDom", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030/?direct=true");
  await page.locator("#large-file-picker").setInputFiles({
    name: "direct.bin", mimeType: "application/octet-stream", buffer: Buffer.from([0, 255, 128]),
  });
  await expect(page.locator("#large-upload")).toHaveText("direct.bin|3|0|128");
  await page.getByRole("button", { name: "Increment" }).click();
  await expect(page.locator("#main")).toContainText("hello axum! 1");
});

test("file uploads work with a cross-origin absolute WebSocket URL", async ({ page }) => {
  await page.goto("http://localhost:3030");
  await expect(page.locator(".onmounted-div")).toHaveText("onmounted was called 1 times");

  const credentials = await page.evaluate(() => {
    const originalFetch = window.fetch;
    let uploadCredentials;
    window.fetch = (input, init) => {
      if (String(input).includes("/ws/upload/")) uploadCredentials = init?.credentials;
      return originalFetch(input, init);
    };
    Object.assign(window, { uploadCredentials: () => uploadCredentials });
  });
  expect(credentials).toBeUndefined();

  await page.locator("#large-file-picker").setInputFiles({
    name: "cross-origin.bin",
    mimeType: "application/octet-stream",
    buffer: Buffer.from([42]),
  });
  await expect(page.locator("#large-upload")).toHaveText("cross-origin.bin|1|42|42");
  expect(await page.evaluate(() => /** @type {any} */ (window).uploadCredentials())).toBe("include");
});

test("retained files count toward only their connection's upload cap", async ({ page, context }) => {
  async function reserve(page, size) {
    return page.evaluate(async (size) => {
      try {
        await window.ipc.beginFileUpload({
          size,
          event: {
            element: 0, name: "change", bubbles: true,
            data: { values: [{ key: "file", file: {
              path: "quota-probe.bin", size, last_modified: 0, content_type: "application/octet-stream",
            } }] },
          },
        });
        window.ipc.postMessage(JSON.stringify({ method: "file_upload_cancel", params: {} }));
        return "accepted";
      } catch (error) {
        return error.message;
      }
    }, size);
  }

  await page.goto("http://127.0.0.1:3030");
  await page.locator("#retained-file-picker").setInputFiles({
    name: "retained.txt", mimeType: "text/plain", buffer: Buffer.from("abc"),
  });
  await expect(page.locator("#retained-files")).toHaveText("1");
  const limit = 1024 * 1024 * 1024;
  expect(await reserve(page, limit - 3)).toBe("accepted");
  expect(await reserve(page, limit - 2)).toContain("upload data limit");

  const other = await context.newPage();
  await other.goto("http://127.0.0.1:3030");
  await expect(other.locator(".onmounted-div")).toHaveText("onmounted was called 1 times");
  expect(await reserve(other, limit)).toBe("accepted");
  await other.close();

  await page.getByRole("button", { name: "Release files" }).click();
  await expect(page.locator("#retained-files")).toHaveText("0");
  expect(await reserve(page, limit)).toBe("accepted");
});

test("a rejected upload leaves the connection usable for events and uploads", async ({ page }) => {
  /** @type {string[]} */
  const errors = [];
  /** @type {Error[]} */
  const unhandled = [];
  let closed = false;
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => unhandled.push(error));
  page.on("websocket", (socket) => {
    socket.on("close", () => { closed = true; });
  });
  await page.goto("http://127.0.0.1:3030");
  const picker = page.locator("#large-file-picker");
  await picker.evaluate((element) => {
    const input = /** @type {HTMLInputElement} */ (element);
    const files = new DataTransfer();
    files.items.add(new File(["x"], "too-large.bin"));
    input.files = files.files;
    // Exceed the default limit without allocating a gigabyte in the browser.
    Object.defineProperty(input.files[0], "size", { value: 1024 * 1024 * 1024 + 1 });
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });

  await expect.poll(() => errors.join("\n")).toContain("exceeds the connection's upload data limit");
  await expect(page.locator("#large-upload")).toHaveText("");
  await page.getByRole("button", { name: "Increment" }).click();
  await expect(page.locator("#main")).toContainText("hello axum! 1");
  await picker.setInputFiles({
    name: "retry.bin", mimeType: "application/octet-stream", buffer: Buffer.from([42]),
  });
  await expect(page.locator("#large-upload")).toHaveText("retry.bin|1|42|42");
  expect(closed).toBe(false);
  expect(unhandled).toEqual([]);
});

test("invalid upload metadata does not block subsequent events or uploads", async ({ page }) => {
  /** @type {string[]} */
  const errors = [];
  /** @type {Error[]} */
  const unhandled = [];
  let closed = false;
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => unhandled.push(error));
  page.on("websocket", (socket) => {
    socket.on("close", () => { closed = true; });
  });
  await page.goto("http://127.0.0.1:3030");
  const picker = page.locator("#large-file-picker");
  await picker.evaluate((element) => {
    const input = /** @type {HTMLInputElement} */ (element);
    const files = new DataTransfer();
    // Browsers allow dates before the Unix epoch, which the server's u64 cannot represent.
    files.items.add(new File(["x"], "old-file.txt", { type: "text/plain", lastModified: -1 }));
    input.files = files.files;
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await page.getByRole("button", { name: "Increment" }).click();

  await expect.poll(() => errors.join("\n")).toContain("invalid file upload metadata");
  await expect(page.locator("#large-upload")).toHaveText("");
  await expect(page.locator("#main")).toContainText("hello axum! 1");
  expect(await page.evaluate(() => window.ipc.pendingFileUpload)).toBeNull();
  await picker.setInputFiles({
    name: "retry.bin", mimeType: "application/octet-stream", buffer: Buffer.from([42]),
  });
  await expect(page.locator("#large-upload")).toHaveText("retry.bin|1|42|42");
  expect(closed).toBe(false);
  expect(unhandled).toEqual([]);
});

test("text edits preserve order without reuploading selected files", async ({ page }) => {
  /** @type {any[]} */
  const events = [];
  page.on("websocket", (socket) => {
    socket.on("framesent", ({ payload }) => {
      const text = payload.toString();
      if (!text.startsWith("{")) return;
      const message = JSON.parse(text);
      if (message.method === "user_event") events.push(message.params);
      if (message.method === "file_upload") events.push(message.params.event);
    });
  });
  await page.goto("http://127.0.0.1:3030");
  await expect(page.locator(".onmounted-div")).toHaveText("onmounted was called 1 times");
  events.length = 0;

  // Delay the first HTTP upload while changing the selection and typing.
  await page.evaluate(() => {
    const original = window.ipc.uploadFile.bind(window.ipc);
    let uploads = 0;
    let release;
    const gate = new Promise((resolve) => { release = resolve; });
    Object.assign(window, {
      fileUploadCount: () => uploads,
      releaseFileUpload: () => release(),
    });
    window.ipc.uploadFile = async function (token, file) {
      if (++uploads === 1) await gate;
      return original(token, file);
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
  await page.evaluate(() => /** @type {any} */ (window).releaseFileUpload());

  await expect(page.locator("#description-values")).toHaveText("a,ab,abc");
  await expect(page.locator("#form-file-picker-counts")).toHaveText("0,2");
  expect(events.map(({ name }) => name)).toEqual(["change", "input", "change", "input", "input"]);
  const changes = events.filter(({ name }) => name === "change");
  expect(changes.every(({ data }) => data.values.every(({ file }) => !file?.contents))).toBe(true);
  const inputs = events.filter(({ name }) => name === "input");
  expect(inputs.every(({ data }) => data.values.every(({ file }) => !file?.contents))).toBe(true);
  expect(await page.evaluate(() => /** @type {any} */ (window).fileUploadCount())).toBe(2);
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

test("a failed HTTP upload does not block subsequent events", async ({ page }) => {
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
    window.ipc.uploadFile = () => Promise.reject(new Error("Upload unavailable"));
  });
  await page.locator("#form-file-picker").setInputFiles({
    name: "unreadable.txt", mimeType: "text/plain", buffer: Buffer.from("hello"),
  });
  await page.locator('input[name="description"]').fill("still responsive");
  await expect(page.locator("#description-values")).toHaveText("still responsive");
  expect(errors.filter((message) => message.includes("Failed to send LiveView event"))).toHaveLength(2);
  expect(unhandled).toEqual([]);
});

test("file events wait for the server to confirm upload completion", async ({ page }) => {
  await page.goto("http://127.0.0.1:3030");
  await expect(page.locator(".onmounted-div")).toHaveText("onmounted was called 1 times");
  await page.evaluate(() => {
    const ws = window.ipc.ws;
    const onmessage = ws.onmessage;
    let held = false;
    ws.onmessage = (message) => {
      const bytes = new Uint8Array(message.data);
      if (!held && bytes[0] === 0 &&
          new TextDecoder().decode(bytes.slice(1)).includes('"file_upload_complete"')) {
        held = true;
        Object.assign(window, { releaseUploadCompletion: () => onmessage(message) });
      } else {
        onmessage(message);
      }
    };
  });
  await page.locator("#file-picker").setInputFiles({
    name: "hello.txt", mimeType: "text/plain", buffer: Buffer.from("hello"),
  });
  await expect.poll(() => page.evaluate(() => typeof window.releaseUploadCompletion)).toBe("function");
  await expect(page.locator("#file-picker-counts")).toHaveText("1,0");
  await page.getByRole("button", { name: "Increment" }).click();
  await expect(page.locator("#main")).toContainText("hello axum! 0");

  await page.evaluate(() => window.releaseUploadCompletion());
  await expect(page.locator("#file-picker-counts")).toHaveText("1,1");
  await expect(page.locator("#main")).toContainText("hello axum! 1");
});

for (const status of [200, 204]) {
  test(`an upload handled by a fallback route keeps the connection usable (${status})`, async ({ page }) => {
    const errors = [];
    const unhandled = [];
    let closed = false;
    page.on("console", (message) => {
      if (message.type() === "error") errors.push(message.text());
    });
    page.on("pageerror", (error) => unhandled.push(error));
    page.on("websocket", (socket) => {
      socket.on("close", () => { closed = true; });
    });
    // A custom router can return success without ever receiving the file into LiveView.
    await page.route("**/ws/upload/*", (route) => route.fulfill({
      status,
      contentType: "text/html",
      body: status === 200 ? "<!doctype html><html>Application fallback</html>" : "",
    }));
    await page.goto("http://127.0.0.1:3030");
    const picker = page.locator("#file-picker");
    const file = { name: "hello.txt", mimeType: "text/plain", buffer: Buffer.from("hello") };
    await picker.setInputFiles(file);
    await expect.poll(() => errors.filter((message) => message.includes("HTTP upload handler")).length).toBe(2);
    await expect(page.locator("#file-picker-counts")).toHaveText("0,0");
    await page.getByRole("button", { name: "Increment" }).click();
    await expect(page.locator("#main")).toContainText("hello axum! 1");

    await page.unroute("**/ws/upload/*");
    await picker.setInputFiles(file);
    await expect(page.locator("#file-picker-text")).toHaveText("hello");
    await expect(page.locator("#file-picker-counts")).toHaveText("1,1");
    expect(closed).toBe(false);
    expect(unhandled).toEqual([]);
    expect(errors).toHaveLength(2);
  });
}

test("the keepalive timer stops when the websocket closes", async ({ page }) => {
  const errors = [];
  let pings = 0;
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("websocket", (socket) => {
    socket.on("framesent", ({ payload }) => {
      if (payload === "__ping__") pings++;
    });
  });
  await page.clock.install();
  await page.goto("http://127.0.0.1:3030");
  await expect(page.locator(".onmounted-div")).toHaveText("onmounted was called 1 times");
  await page.clock.runFor(30000);
  await expect.poll(() => pings).toBe(1);
  await page.evaluate(() => window.ipc.ws.close());
  await expect.poll(() => page.evaluate(() => window.ipc.ws.readyState)).toBe(3);
  await page.clock.runFor(60000);
  expect(pings).toBe(1);
  expect(errors).toEqual([]);
});
