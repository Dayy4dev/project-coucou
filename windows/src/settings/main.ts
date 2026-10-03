// Settings window — the place where anything that writes to disk is confirmed.
// Stage 2 covers the Claude Code hooks and the general preferences; API keys and
// integrations land here too in a later stage.

import "./settings.css";
import { Bridge, onEvent, type HookStatus } from "../core/bridge";
import { DEFAULT_SETTINGS, type Settings, type MouseAction, type ChatProvider } from "../core/state";
import { h, clear } from "../views/dom";

let settings: Settings = { ...DEFAULT_SETTINGS };
let version = "";

const root = document.getElementById("settings-root")!;

async function save() {
  await Bridge.saveSettings(settings);
}

// ── Reusable bits ─────────────────────────────────────────────────────────────

function toggle(on: boolean, onChange: (v: boolean) => void): HTMLElement {
  const el = h("button", { class: on ? "switch on" : "switch", "aria-pressed": on });
  el.addEventListener("click", () => {
    const next = !el.classList.contains("on");
    el.classList.toggle("on", next);
    onChange(next);
  });
  return el;
}

function statusDot(ok: boolean): HTMLElement {
  return h("i", { class: "dot", style: `background:${ok ? "#22c55e" : "#f4505e"}` });
}

function renderDiff(text: string): HTMLElement {
  const box = h("div", { class: "diff" });
  for (const line of text.split("\n")) {
    const cls = line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : "ctx";
    box.append(h("div", { class: cls, text: line }));
  }
  return box;
}

// ── Claude Code section ───────────────────────────────────────────────────────

function claudeSection(status: HookStatus): HTMLElement {
  const body = h("div", { style: "display:flex;flex-direction:column;gap:12px" });
  const section = h(
    "section",
    {},
    h("h2", {}, statusDot(status.installed), h("span", { text: "Claude Code" })),
    body,
  );

  const rebuild = async () => {
    const fresh = await Bridge.hooksStatus();
    if (fresh) Object.assign(status, fresh);
    clear(body);
    draw();
    const head = section.querySelector("h2")!;
    clear(head);
    head.append(statusDot(status.installed), h("span", { text: "Claude Code" }));
  };

  function draw() {
    body.append(
      h("div", {
        class: "hint",
        text: status.installed
          ? "Coucou is hooked into your Claude Code sessions. Tool calls, questions and permission requests show up in the island, and you can answer them there."
          : "Install the hooks to see your Claude Code sessions in the island and approve permissions without leaving what you are doing.",
      }),
      h("div", { class: "row" },
        h("label", { text: "settings.json" }),
        h("span", { class: "path", text: status.settingsPath }),
      ),
      h("div", { class: "row" },
        h("label", { text: "Relay" }),
        h("span", { class: "path", text: status.hookPath }),
        statusDot(status.hookReady),
      ),
    );

    if (!status.hookReady) {
      body.append(h("div", {
        class: "notice warn",
        text: "coucou-hook.exe is not in place yet. Restart Coucou; if it still fails, build it with `cargo build -p coucou-hook`.",
      }));
    }

    const actions = h("div", { class: "row" });
    const install = h("button", {
      class: "primary",
      text: status.installed ? "Reinstall hooks…" : "Install hooks…",
      onclick: () => showPreview(true),
    });
    // Writing hook commands that point at a relay which isn't there would give
    // every Claude Code session a broken hook and nothing to show for it.
    if (!status.hookReady) {
      install.disabled = true;
      install.title = "The relay isn't installed yet.";
    }
    actions.append(install);
    if (status.installed) {
      actions.append(h("button", {
        class: "danger",
        text: "Uninstall hooks…",
        onclick: () => showPreview(false),
      }));
    }
    body.append(actions);
  }

  async function showPreview(install: boolean) {
    let preview;
    try {
      preview = await Bridge.hooksPreview(install);
    } catch (err) {
      // An unreadable or invalid settings.json stops here rather than being
      // treated as empty and written over.
      clear(body);
      body.append(
        h("div", { class: "notice err", text: String(err).replace(/^Error:\s*/, "") }),
        h("div", { class: "row" }, h("button", {
          text: "Back",
          onclick: () => { clear(body); draw(); },
        })),
      );
      return;
    }
    if (!preview) return;
    clear(body);
    body.append(
      h("div", {
        class: "hint",
        text: install
          ? "This is exactly what will change in your settings.json. Your own hooks are left untouched."
          : "This removes Coucou's entries only. Your own hooks are left untouched.",
      }),
      renderDiff(preview.diff),
      h("div", { class: "row" },
        h("span", { class: "path", text: `Backup → ${preview.backup}` }),
      ),
    );
    const confirm = h("button", {
      class: install ? "primary" : "danger",
      text: install ? "Back up and write" : "Back up and remove",
    });
    confirm.addEventListener("click", async () => {
      confirm.disabled = true;
      try {
        const backup = await Bridge.hooksApply(install, preview.fingerprint);
        clear(body);
        body.append(h("div", {
          class: "notice ok",
          text: `Done. Previous settings saved as ${backup}. Open a new Claude Code session to pick the hooks up.`,
        }));
        window.setTimeout(() => void rebuild(), 2600);
      } catch (err) {
        confirm.disabled = false;
        body.append(h("div", { class: "notice err", text: `Could not write: ${String(err)}` }));
      }
    });
    body.append(h("div", { class: "row" }, confirm, h("button", {
      text: "Cancel",
      onclick: () => { clear(body); draw(); },
    })));
  }

  draw();
  return section;
}

// ── Chat & AI Provider section ───────────────────────────────────────────────

const ANTHROPIC_PRESET_MODELS: [string, string][] = [
  ["claude-opus-5", "Claude Opus 5"],
  ["claude-sonnet-5", "Claude Sonnet 5"],
  ["claude-haiku-4-5", "Claude Haiku 4.5"],
];

const OPENAI_PRESET_MODELS: [string, string][] = [
  ["gpt-4o", "GPT-4o"],
  ["gpt-4o-mini", "GPT-4o mini"],
  ["o3-mini", "o3 mini"],
  ["gpt-4-turbo", "GPT-4 Turbo"],
];

function apiSection(hasAnthropicKey: boolean, hasOpenaiKey: boolean): HTMLElement {
  const section = h("section", {});
  const dot = statusDot(false);
  const feedback = h("div", {});
  const body = h("div", { style: "display:flex;flex-direction:column;gap:12px" });

  let keyPresent = {
    anthropic: hasAnthropicKey,
    openai: hasOpenaiKey,
  };

  async function refreshKeyStatus() {
    keyPresent.anthropic = (await Bridge.secretPresent("anthropic-api-key")) ?? false;
    keyPresent.openai = (await Bridge.secretPresent("openai-api-key")) ?? false;
    const ok = settings.chatProvider === "anthropic" ? keyPresent.anthropic : keyPresent.openai;
    dot.style.background = ok ? "#22c55e" : "#f4505e";
  }

  function render() {
    clear(body);
    clear(feedback);
    const provider = settings.chatProvider || "anthropic";
    const ok = provider === "anthropic" ? keyPresent.anthropic : keyPresent.openai;
    dot.style.background = ok ? "#22c55e" : "#f4505e";

    // 1. Provider selector row
    const providerSelect = h("select", {}) as HTMLSelectElement;
    providerSelect.append(
      h("option", { value: "anthropic", text: "Anthropic (Claude)" }),
      h("option", { value: "openai", text: "OpenAI-compatible (Custom Base URL)" }),
    );
    providerSelect.value = provider;
    providerSelect.addEventListener("change", () => {
      settings.chatProvider = providerSelect.value as ChatProvider;
      void save();
      void Bridge.chatReset();
      render();
    });

    body.append(
      h("div", { class: "row" },
        h("label", { text: "Provider" }),
        providerSelect,
        h("span", {
          class: "hint",
          text: provider === "anthropic" ? "Anthropic Messages API" : "Any endpoint that speaks POST /chat/completions",
        }),
      ),
    );

    if (provider === "anthropic") {
      renderAnthropicControls();
    } else {
      renderOpenAIControls();
    }
  }

  function renderAnthropicControls() {
    const hasKey = keyPresent.anthropic;
    const keyField = h("input", {
      type: "password",
      placeholder: hasKey ? "••••••••••••  (stored)" : "sk-ant-...",
      style: "flex:1 1 auto;min-width:0",
      autocomplete: "off",
      spellcheck: "false",
    }) as HTMLInputElement;

    const saveKeyBtn = h("button", { class: "primary", text: "Save key" });
    const clearKeyBtn = h("button", { class: "danger", text: "Remove", style: hasKey ? "" : "display:none" });

    saveKeyBtn.addEventListener("click", async () => {
      const value = keyField.value.trim();
      if (!value) return;
      clear(feedback);
      try {
        await Bridge.secretSet("anthropic-api-key", value);
        keyField.value = "";
        feedback.append(h("div", { class: "notice ok", text: "Saved in Windows Credential Manager." }));
        await refreshKeyStatus();
        render();
      } catch (err) {
        feedback.append(h("div", { class: "notice err", text: `Could not save: ${String(err)}` }));
      }
    });

    clearKeyBtn.addEventListener("click", async () => {
      clear(feedback);
      try {
        await Bridge.secretClear("anthropic-api-key");
        feedback.append(h("div", { class: "notice ok", text: "Key removed." }));
        await refreshKeyStatus();
        render();
      } catch (err) {
        feedback.append(h("div", { class: "notice err", text: `Could not remove: ${String(err)}` }));
      }
    });

    const modelSelect = h("select", { style: "min-width:180px" }) as HTMLSelectElement;
    for (const [id, label] of ANTHROPIC_PRESET_MODELS) {
      modelSelect.append(h("option", { value: id, text: label }));
    }
    if (!ANTHROPIC_PRESET_MODELS.some(([id]) => id === settings.model)) {
      modelSelect.append(h("option", { value: settings.model, text: settings.model }));
    }
    modelSelect.value = settings.model;
    modelSelect.addEventListener("change", () => {
      settings.model = modelSelect.value;
      void save();
    });

    const autoDetectBtn = h("button", { text: "Auto-detect" });
    autoDetectBtn.addEventListener("click", async () => {
      autoDetectBtn.disabled = true;
      autoDetectBtn.textContent = "Detecting…";
      clear(feedback);
      try {
        const models = await Bridge.fetchModels("anthropic");
        if (models && models.length > 0) {
          clear(modelSelect);
          for (const m of models) {
            modelSelect.append(h("option", { value: m.id, text: m.label }));
          }
          if (models.some((m) => m.id === settings.model)) {
            modelSelect.value = settings.model;
          } else {
            modelSelect.value = models[0].id;
            settings.model = models[0].id;
            void save();
          }
          feedback.append(h("div", { class: "notice ok", text: `Detected ${models.length} Anthropic models.` }));
        } else {
          feedback.append(h("div", { class: "notice err", text: "No models returned by Anthropic." }));
        }
      } catch (err) {
        feedback.append(h("div", { class: "notice err", text: `Auto-detect failed: ${String(err)}` }));
      } finally {
        autoDetectBtn.disabled = false;
        autoDetectBtn.textContent = "Auto-detect";
      }
    });

    body.append(
      h("div", { class: "row" },
        h("label", { text: "API key" }),
        keyField,
        saveKeyBtn,
        clearKeyBtn,
      ),
      h("div", { class: "row" },
        h("label", { text: "Model" }),
        modelSelect,
        autoDetectBtn,
      ),
    );
  }

  function renderOpenAIControls() {
    const hasKey = keyPresent.openai;

    // Base URL input
    const urlField = h("input", {
      type: "text",
      value: settings.openaiBaseUrl || "https://api.openai.com/v1",
      placeholder: "https://api.openai.com/v1",
      style: "flex:1 1 auto;min-width:0",
      spellcheck: "false",
    }) as HTMLInputElement;

    urlField.addEventListener("change", () => {
      settings.openaiBaseUrl = urlField.value.trim() || "https://api.openai.com/v1";
      void save();
    });

    // API Key input
    const keyField = h("input", {
      type: "password",
      placeholder: hasKey ? "••••••••••••  (stored)" : "sk-... (optional for local models)",
      style: "flex:1 1 auto;min-width:0",
      autocomplete: "off",
      spellcheck: "false",
    }) as HTMLInputElement;

    const saveKeyBtn = h("button", { class: "primary", text: "Save key" });
    const clearKeyBtn = h("button", { class: "danger", text: "Remove", style: hasKey ? "" : "display:none" });

    saveKeyBtn.addEventListener("click", async () => {
      const value = keyField.value.trim();
      clear(feedback);
      try {
        await Bridge.secretSet("openai-api-key", value);
        keyField.value = "";
        feedback.append(h("div", { class: "notice ok", text: "Saved in Windows Credential Manager." }));
        await refreshKeyStatus();
        render();
      } catch (err) {
        feedback.append(h("div", { class: "notice err", text: `Could not save: ${String(err)}` }));
      }
    });

    clearKeyBtn.addEventListener("click", async () => {
      clear(feedback);
      try {
        await Bridge.secretClear("openai-api-key");
        feedback.append(h("div", { class: "notice ok", text: "Key removed." }));
        await refreshKeyStatus();
        render();
      } catch (err) {
        feedback.append(h("div", { class: "notice err", text: `Could not remove: ${String(err)}` }));
      }
    });

    // Model selection: select dropdown + manual entry option
    const modelSelect = h("select", { style: "min-width:180px" }) as HTMLSelectElement;
    for (const [id, label] of OPENAI_PRESET_MODELS) {
      modelSelect.append(h("option", { value: id, text: label }));
    }
    const currentModel = settings.openaiModel || "gpt-4o";
    if (!OPENAI_PRESET_MODELS.some(([id]) => id === currentModel)) {
      modelSelect.append(h("option", { value: currentModel, text: currentModel }));
    }
    modelSelect.value = currentModel;
    modelSelect.addEventListener("change", () => {
      settings.openaiModel = modelSelect.value;
      customModelInput.value = modelSelect.value;
      void save();
    });

    const customModelInput = h("input", {
      type: "text",
      value: currentModel,
      placeholder: "or type custom model name",
      style: "width:180px",
      spellcheck: "false",
    }) as HTMLInputElement;

    customModelInput.addEventListener("change", () => {
      const val = customModelInput.value.trim();
      if (!val) return;
      settings.openaiModel = val;
      if (![...modelSelect.options].some((o) => o.value === val)) {
        modelSelect.append(h("option", { value: val, text: val }));
      }
      modelSelect.value = val;
      void save();
    });

    const autoDetectBtn = h("button", { text: "Auto-detect" });
    autoDetectBtn.addEventListener("click", async () => {
      autoDetectBtn.disabled = true;
      autoDetectBtn.textContent = "Detecting…";
      clear(feedback);
      try {
        const baseUrl = urlField.value.trim() || settings.openaiBaseUrl;
        const models = await Bridge.fetchModels("openai", baseUrl);
        if (models && models.length > 0) {
          clear(modelSelect);
          for (const m of models) {
            modelSelect.append(h("option", { value: m.id, text: m.label }));
          }
          if (models.some((m) => m.id === settings.openaiModel)) {
            modelSelect.value = settings.openaiModel;
          } else {
            modelSelect.value = models[0].id;
            settings.openaiModel = models[0].id;
            customModelInput.value = models[0].id;
            void save();
          }
          feedback.append(h("div", { class: "notice ok", text: `Detected ${models.length} models from ${baseUrl}.` }));
        } else {
          feedback.append(h("div", { class: "notice err", text: `No models returned by ${baseUrl}/models.` }));
        }
      } catch (err) {
        feedback.append(h("div", { class: "notice err", text: `Auto-detect failed: ${String(err)}` }));
      } finally {
        autoDetectBtn.disabled = false;
        autoDetectBtn.textContent = "Auto-detect";
      }
    });

    body.append(
      h("div", { class: "row" },
        h("label", { text: "Base URL" }),
        urlField,
        h("span", { class: "hint", text: "e.g. http://localhost:11434/v1 for Ollama" }),
      ),
      h("div", { class: "row" },
        h("label", { text: "API key" }),
        keyField,
        saveKeyBtn,
        clearKeyBtn,
      ),
      h("div", { class: "row" },
        h("label", { text: "Model" }),
        modelSelect,
        customModelInput,
        autoDetectBtn,
      ),
    );
  }

  render();

  section.append(
    h("h2", {}, dot, h("span", { text: "Chat & AI Provider" })),
    h("span", { class: "hint", text: "Power the island chat using Anthropic Claude or any OpenAI-compatible server." }),
    body,
    feedback,
  );
  return section;
}

// ── Integrations section ──────────────────────────────────────────────────────

interface IntegrationDef {
  id: string;
  name: string;
  color: string;
  /** Credential Manager keys, in the order they are shown. */
  fields: { key: string; label: string; placeholder: string; secret: boolean }[];
}

const INTEGRATIONS: IntegrationDef[] = [
  { id: "integration_stripe", name: "Stripe", color: "#0570DE",
    fields: [{ key: "stripe-api-key", label: "Secret key", placeholder: "sk_live_…", secret: true }] },
  { id: "integration_github", name: "GitHub", color: "#F4505E",
    fields: [{ key: "github-token", label: "Token", placeholder: "ghp_…", secret: true }] },
  { id: "integration_vercel", name: "Vercel", color: "#7C5CFF",
    fields: [{ key: "vercel-token", label: "Token", placeholder: "…", secret: true }] },
  { id: "integration_n8n", name: "n8n", color: "#F29B38",
    fields: [
      { key: "n8n-url", label: "Instance URL", placeholder: "https://n8n.example.com", secret: false },
      { key: "n8n-api-key", label: "API key", placeholder: "…", secret: true },
    ] },
  { id: "integration_resend", name: "Resend", color: "#22C55E",
    fields: [{ key: "resend-api-key", label: "API key", placeholder: "re_…", secret: true }] },
  { id: "integration_notion", name: "Notion", color: "#8C8C8C",
    fields: [{ key: "notion-api-key", label: "Integration token", placeholder: "ntn_…", secret: true }] },
  { id: "integration_calcom", name: "Cal.com", color: "#C9956A",
    fields: [{ key: "calcom-api-key", label: "API key", placeholder: "cal_…", secret: true }] },
];

const MAX_ACTIVE = 4;

function integrationsSection(present: Record<string, boolean>): HTMLElement {
  const note = h("div", { class: "hint" });
  const list = h("div", { style: "display:flex;flex-direction:column;gap:14px" });

  function updateNote() {
    const used = settings.activeIntegrations.length;
    note.textContent = `Pick up to ${MAX_ACTIVE} pills to show next to Mochi — ${used}/${MAX_ACTIVE} in use. Keys are stored in the Windows Credential Manager, never on disk.`;
  }

  for (const def of INTEGRATIONS) {
    const active = settings.activeIntegrations.includes(def.id);
    const sw = h("button", { class: active ? "switch on" : "switch" });
    sw.addEventListener("click", () => {
      const on = settings.activeIntegrations.includes(def.id);
      if (on) {
        settings.activeIntegrations = settings.activeIntegrations.filter((x) => x !== def.id);
      } else {
        if (settings.activeIntegrations.length >= MAX_ACTIVE) return;
        settings.activeIntegrations = [...settings.activeIntegrations, def.id];
      }
      sw.classList.toggle("on", !on);
      updateNote();
      void save();
    });

    const rows = h("div", { style: "display:flex;flex-direction:column;gap:6px;flex:1 1 auto;min-width:0" });
    for (const field of def.fields) {
      const input = h("input", {
        type: field.secret ? "password" : "text",
        placeholder: present[field.key] ? "••••••••  (stored)" : field.placeholder,
        autocomplete: "off",
        spellcheck: "false",
        style: "flex:1 1 auto;min-width:0",
      }) as HTMLInputElement;
      const saveBtn = h("button", { text: "Save" });
      const dotEl = statusDot(present[field.key] ?? false);
      saveBtn.addEventListener("click", async () => {
        const value = input.value.trim();
        try {
          await Bridge.secretSet(field.key, value);
          present[field.key] = value.length > 0;
          input.value = "";
          input.placeholder = value ? "••••••••  (stored)" : field.placeholder;
          dotEl.style.background = value ? "#22c55e" : "#f4505e";
        } catch {
          dotEl.style.background = "#f5a524";
        }
      });
      rows.append(
        h("div", { class: "row" },
          h("label", { style: "min-width:104px", text: field.label }),
          input, saveBtn, dotEl,
        ),
      );
    }

    list.append(
      h("div", { style: "display:flex;gap:12px;align-items:flex-start" },
        h("div", { style: "display:flex;align-items:center;gap:8px;min-width:132px;padding-top:4px" },
          sw,
          h("i", { class: "dot", style: `background:${def.color}` }),
          h("span", { style: "font-size:12.5px", text: def.name }),
        ),
        rows,
      ),
    );
  }

  updateNote();
  return h("section", {}, h("h2", {}, h("span", { text: "Integrations" })), note, list);
}

// ── General section ───────────────────────────────────────────────────────────

function generalSection(): HTMLElement {
  const volume = h("input", {
    type: "range", min: "0", max: "0.2", step: "0.005",
    value: String(settings.soundVolume),
  }) as HTMLInputElement;
  volume.addEventListener("input", () => {
    settings.soundVolume = Number(volume.value);
    void save();
  });

  const autoClose = h("input", {
    type: "number", min: "5", max: "120", step: "1",
    value: String(Math.round(settings.autoCloseInterval)),
    style: "width:72px",
  }) as HTMLInputElement;
  autoClose.addEventListener("change", () => {
    settings.autoCloseInterval = Math.max(5, Math.min(120, Number(autoClose.value) || 15));
    autoClose.value = String(settings.autoCloseInterval);
    void save();
  });

  const screen = h("select", {}) as HTMLSelectElement;
  screen.append(
    h("option", { value: "primary", text: "Main display" }),
    h("option", { value: "cursor", text: "Display under the cursor" }),
  );
  screen.value = settings.screen;
  screen.addEventListener("change", () => {
    settings.screen = screen.value as Settings["screen"];
    void save();
  });

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "General" })),
    h("div", { class: "row" },
      h("label", { text: "Sound" }),
      toggle(settings.soundEnabled, (v) => { settings.soundEnabled = v; void save(); }),
      volume,
    ),
    h("div", { class: "row" },
      h("label", { text: "Auto-close" }),
      autoClose,
      h("span", { class: "hint", text: "seconds after you leave the island" }),
    ),
    h("div", { class: "row" },
      h("label", { text: "Island lives on" }),
      screen,
    ),
    h("div", { class: "row" },
      h("label", { text: "Launch at startup" }),
      toggle(settings.autostart, (v) => { settings.autostart = v; void save(); }),
    ),
  );
}

// ── Controls section (shortcut + mouse gestures) ──────────────────────────────

const HOTKEY_PRESETS: [string, string][] = [
  ["Ctrl+Shift+C", "Ctrl + Shift + C"],
  ["Ctrl+Alt+C", "Ctrl + Alt + C"],
  ["Ctrl+Shift+Space", "Ctrl + Shift + Space"],
  ["Alt+C", "Alt + C"],
  ["Ctrl+Shift+N", "Ctrl + Shift + N"],
  ["Ctrl+Shift+F12", "Ctrl + Shift + F12"],
];

function actionSelect(current: MouseAction, onChange: (v: MouseAction) => void): HTMLSelectElement {
  const sel = h("select", {}) as HTMLSelectElement;
  const options: [MouseAction, string][] = [
    ["toggle", "Open / close"],
    ["hide", "Close only"],
    ["none", "Do nothing"],
  ];
  for (const [id, label] of options) sel.append(h("option", { value: id, text: label }));
  sel.value = current;
  sel.addEventListener("change", () => onChange(sel.value as MouseAction));
  return sel;
}

function controlsSection(): HTMLElement {
  // The shortcut row is only meaningful while the hotkey is on.
  const hotkeyRow = h("div", { class: "row" });
  const shortcutSelect = h("select", {}) as HTMLSelectElement;
  for (const [id, label] of HOTKEY_PRESETS) shortcutSelect.append(h("option", { value: id, text: label }));
  if (!HOTKEY_PRESETS.some(([id]) => id === settings.hotkey)) {
    shortcutSelect.append(h("option", { value: settings.hotkey, text: settings.hotkey }));
  }
  shortcutSelect.value = settings.hotkey;
  shortcutSelect.addEventListener("change", () => {
    settings.hotkey = shortcutSelect.value;
    void save();
  });

  const refreshHotkeyRow = () => {
    clear(hotkeyRow);
    if (settings.hotkeyEnabled) {
      hotkeyRow.append(
        h("label", { text: "Shortcut" }),
        shortcutSelect,
        h("span", { class: "hint", text: "presses this → open or close the island" }),
      );
    }
  };

  const hotkeyToggle = toggle(settings.hotkeyEnabled, (v) => {
    settings.hotkeyEnabled = v;
    refreshHotkeyRow();
    void save();
  });

  refreshHotkeyRow();

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Controls" })),
    h("div", { class: "hint", text: "Open or close the island without reaching for the top of the screen." }),
    h("div", { class: "row" },
      h("label", { text: "Keyboard shortcut" }),
      hotkeyToggle,
    ),
    hotkeyRow,
    h("div", { class: "row" },
      h("label", { text: "Right-click island" }),
      actionSelect(settings.rightClickAction, (v) => { settings.rightClickAction = v; void save(); }),
    ),
    h("div", { class: "row" },
      h("label", { text: "Middle-click island" }),
      actionSelect(settings.middleClickAction, (v) => { settings.middleClickAction = v; void save(); }),
    ),
    h("div", { class: "hint", text: "A click on the thin strip at the top of the screen always opens the island." }),
  );
}

// ── Boot ──────────────────────────────────────────────────────────────────────

async function main() {
  const boot = await Bridge.boot();
  if (boot) {
    settings = { ...settings, ...boot.settings };
    version = boot.version;
  }
  const status = (await Bridge.hooksStatus()) ?? {
    installed: false, settingsPath: "", hookPath: "", hookReady: false,
  };

  const hasAnthropicKey = (await Bridge.secretPresent("anthropic-api-key")) ?? false;
  const hasOpenaiKey = (await Bridge.secretPresent("openai-api-key")) ?? false;

  const keys = [
    "stripe-api-key", "github-token", "vercel-token",
    "n8n-url", "n8n-api-key", "resend-api-key", "notion-api-key", "calcom-api-key",
  ];
  const present: Record<string, boolean> = {};
  for (const k of keys) present[k] = (await Bridge.secretPresent(k)) ?? false;

  clear(root);
  root.append(
    h("h1", {}, h("span", { text: "Coucou" }), h("span", { class: "version", text: version })),
    claudeSection(status),
    apiSection(hasAnthropicKey, hasOpenaiKey),
    integrationsSection(present),
    controlsSection(),
    generalSection(),
    h("div", {
      class: "hint",
      text: "No telemetry. Network requests only go to the services you configure yourself.",
    }),
  );

  void onEvent<Settings>("settings-changed", (s) => {
    settings = { ...settings, ...s };
  });
}

void main();
