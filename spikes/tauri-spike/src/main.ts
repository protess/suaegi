import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Channel, invoke } from "@tauri-apps/api/core";
import "@xterm/xterm/css/xterm.css";

async function main() {
  const term = new Terminal({
    fontSize: 14,
    fontFamily: "Menlo, Monaco, monospace",
    scrollback: 5000,
    theme: {
      background: "#1e1e2e",
      foreground: "#cdd6f4",
    },
  });

  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(document.getElementById("terminal")!);

  try {
    term.loadAddon(new WebglAddon());
    console.log("renderer: webgl");
  } catch (e) {
    console.warn("webgl unavailable, falling back to canvas/dom", e);
  }

  fit.fit();

  const onData = new Channel<ArrayBuffer>();
  onData.onmessage = (buf) => term.write(new Uint8Array(buf));

  await invoke("start_pty", { onData, rows: term.rows, cols: term.cols });

  term.onData((data) => invoke("pty_write", { data }));

  window.addEventListener("resize", () => {
    fit.fit();
    invoke("pty_resize", { rows: term.rows, cols: term.cols });
  });

  term.focus();
}

main();
