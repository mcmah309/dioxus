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
      for (const upload of this.pendingFileUploads.values()) {
        upload.reject(
          new Error("LiveView websocket closed during a file upload")
        );
      }
      this.pendingFileUploads.clear();
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
            case "file_download":
              this.downloadFile(event.data.token);
              break;
            case "file_upload":
            case "file_upload_complete":
            case "file_upload_error": {
              const upload = this.pendingFileUploads.get(event.data.id);
              if (!upload) {
                throw new Error("Received an unexpected LiveView file upload response");
              }
              this.pendingFileUploads.delete(event.data.id);
              if (event.type === "file_upload_error") {
                upload.reject(new Error(event.data.error));
              } else {
                upload.resolve(event.data);
              }
              break;
            }
          }
        }
      }
    };

    this.ws = ws;
    this.pendingFileUploads = new Map();
    this.nextFileUploadId = 0;
  }

  postMessage(msg) {
    this.ws.send(msg);
  }

  beginFileUpload(params) {
    return this.requestFileUpload("file_upload", { ...params, id: this.nextFileUploadId++ });
  }

  completeFileUpload(id) {
    return this.requestFileUpload("file_upload_complete", { id });
  }

  cancelFileUpload(id) {
    this.postMessage(JSON.stringify({ method: "file_upload_cancel", params: { id } }));
  }

  downloadFile(token) {
    const url = new URL(this.ws.url);
    url.protocol = url.protocol === "wss:" ? "https:" : "http:";
    url.pathname = `${url.pathname.replace(/\/$/, "")}/download/${encodeURIComponent(token)}`;
    url.hash = "";
    const link = document.createElement("a");
    link.href = url.toString();
    link.download = "";
    // An HTTP error (or a missing route returning a fallback page) must not navigate away
    // from the application. Let the browser stream attachments through its download manager.
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    link.hidden = true;
    document.body.appendChild(link);
    link.click();
    link.remove();
  }

  requestFileUpload(method, params) {
    if (this.pendingFileUploads.has(params.id)) {
      return Promise.reject(new Error("A request for this LiveView file upload is already pending"));
    }
    if (this.ws.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error("LiveView websocket is not open"));
    }
    return new Promise((resolve, reject) => {
      this.pendingFileUploads.set(params.id, { resolve, reject });
      try {
        this.ws.send(JSON.stringify({ method, params }));
      } catch (error) {
        this.pendingFileUploads.delete(params.id);
        reject(error);
      }
    });
  }

  async uploadFile(token, file, signal) {
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
      signal,
    });
    if (!response.ok) {
      throw new Error(`LiveView file upload failed with status ${response.status}`);
    }
  }
}

main();
