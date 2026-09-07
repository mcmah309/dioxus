const intercept_link_redirects = false;

function main() {
  let root = window.document.getElementById("main");
  if (root != null) {
    window.ipc = new IPC(root);
  }
}

class IPC {
  constructor(root) {
    window.interpreter = new NativeInterpreter();
    window.interpreter.initialize(root);
    window.interpreter.liveview = true;
    window.interpreter.ipc = this;
    const ws = new WebSocket(WS_ADDR);
    ws.binaryType = "arraybuffer";
    let pingInterval;

    function ping() {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send("__ping__");
      }
    }

    ws.onopen = () => {
      // we ping every 30 seconds to keep the websocket alive
      pingInterval = setInterval(ping, 30000);
    };

    ws.onerror = (err) => {
      // todo: retry the connection
    };

    ws.onclose = () => {
      clearInterval(pingInterval);
      if (this.pendingFileUpload) {
        this.pendingFileUpload.reject(
          new Error("LiveView websocket closed during a file upload")
        );
        this.pendingFileUpload = null;
      }
    };

    ws.onmessage = (message) => {
      const u8view = new Uint8Array(message.data);
      const binaryFrame = u8view[0] == 1;
      const messageData = message.data.slice(1);
      // The first byte tells the shim if this is a binary of text frame
      if (binaryFrame) {
        // binary frame
        window.interpreter.run_from_bytes(messageData);
      } else {
        // text frame
        let decoder = new TextDecoder("utf-8");

        // Using decode method to get string output
        let str = decoder.decode(messageData);
        // Ignore pongs
        if (str != "__pong__") {
          const event = JSON.parse(str);
          switch (event.type) {
            case "query":
              Function("Eval", `"use strict";${event.data};`)();
              break;
            case "file_upload":
            case "file_upload_complete":
            case "file_upload_error":
              if (!this.pendingFileUpload) {
                throw new Error("Received an unexpected LiveView file upload response");
              }
              if (event.type === "file_upload_error") {
                this.pendingFileUpload.reject(new Error(event.data));
              } else {
                this.pendingFileUpload.resolve(event.data);
              }
              this.pendingFileUpload = null;
              break;
          }
        }
      }
    };

    this.ws = ws;
    this.pendingFileUpload = null;
  }

  postMessage(msg) {
    this.ws.send(msg);
  }

  beginFileUpload(params) {
    return this.requestFileUpload("file_upload", params);
  }

  completeFileUpload() {
    return this.requestFileUpload("file_upload_complete", {});
  }

  requestFileUpload(method, params) {
    if (this.pendingFileUpload) {
      return Promise.reject(new Error("A LiveView file upload is already pending"));
    }
    if (this.ws.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error("LiveView websocket is not open"));
    }
    return new Promise((resolve, reject) => {
      this.pendingFileUpload = { resolve, reject };
      try {
        this.ws.send(JSON.stringify({ method, params }));
      } catch (error) {
        this.pendingFileUpload = null;
        reject(error);
      }
    });
  }

  async uploadFile(token, file) {
    const url = new URL(this.ws.url);
    url.protocol = url.protocol === "wss:" ? "https:" : "http:";
    url.pathname = `${url.pathname.replace(/\/$/, "")}/upload/${encodeURIComponent(token)}`;
    url.hash = "";
    const contentLength = file.size.toString();
    const response = await fetch(url, {
      method: "PUT",
      credentials: "include",
      headers: {
        "Content-Type": file.type,
        "Content-Length": contentLength,
        "X-Content-Size": contentLength,
        "Content-Disposition": `attachment; filename="${escape(file.name)}"`,
        "X-Request-Client": "dioxus",
      },
      body: file,
    });
    if (!response.ok) {
      throw new Error(`LiveView file upload failed with status ${response.status}`);
    }
  }
}

main();
