(function () {
  const appShell = document.getElementById("app-shell");
  const settingsPage = document.getElementById("settings-page");
  const messageList = document.getElementById("message-list");
  const composer = document.getElementById("composer");
  const input = document.getElementById("message-input");
  const sendButton = document.getElementById("send-button");
  const openSettings = document.getElementById("open-settings");
  const settingsBack = document.getElementById("settings-back");
  const settingsForm = document.getElementById("settings-form");
  const cfgAppId = document.getElementById("cfg-app-id");
  const cfgAppSecret = document.getElementById("cfg-app-secret");
  const cfgWikiToken = document.getElementById("cfg-wiki-token");
  const settingsHint = document.getElementById("settings-hint");
  const settingsStatus = document.getElementById("settings-status");
  const testButton = document.getElementById("btn-test");
  const saveButton = document.getElementById("btn-save");

  const messages = new Map();
  let currentConfig = null;
  let historyLoaded = false;
  let messageListWasAtBottom = true;
  const tauri = window.__TAURI__;
  const invoke = tauri && tauri.core && tauri.core.invoke;
  const listen = tauri && tauri.event && tauri.event.listen;

  var statusSvg = {
    queued:
      '<svg class="status-icon status-icon--syncing" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.5" stroke-dasharray="20 12" /></svg>',
    synced:
      '<svg class="status-icon" viewBox="0 0 16 16"><path d="M4 8.5l3 3 5-6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    failed:
      '<svg class="status-icon" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 5v4M8 10.5v.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>',
  };

  var statusLabel = {
    queued: "同步中",
    synced: "已同步",
    failed: "同步失败，点击重试",
  };

  function isTauriReady() {
    return typeof invoke === "function";
  }

  function normalizeMessage(raw) {
    return {
      id: String(raw.id),
      text: String(raw.text || ""),
      created_at: raw.created_at || new Date().toISOString(),
      sync_status: raw.sync_status || raw.status || "queued",
      error_reason: raw.error_reason || null,
    };
  }

  function formatTime(value) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) {
      return "--:--";
    }

    return date.toLocaleTimeString("zh-CN", {
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    });
  }

  function createElement(tagName, className, textContent) {
    const element = document.createElement(tagName);
    if (className) {
      element.className = className;
    }
    if (textContent !== undefined) {
      element.textContent = textContent;
    }
    return element;
  }

  function showChat() {
    appShell.style.display = "";
    settingsPage.style.display = "none";
    if (!historyLoaded) {
      loadHistory();
      historyLoaded = true;
    }
    input.focus();
  }

  function showSettings() {
    appShell.style.display = "none";
    settingsPage.style.display = "grid";
    cfgAppId.focus();
  }

  function setSettingsStatus(text, kind) {
    settingsStatus.textContent = text || "";
    settingsStatus.classList.remove("settings-status--success", "settings-status--error");
    if (kind) {
      settingsStatus.classList.add("settings-status--" + kind);
    }
  }

  function fillSettingsForm(config) {
    currentConfig = config || {};
    cfgAppId.value = currentConfig.app_id || "";
    cfgAppSecret.value = "";
    cfgAppSecret.placeholder = currentConfig.app_secret_hint || "输入 App Secret";
    cfgWikiToken.value = currentConfig.wiki_node_token || "";

    const readonly = Boolean(currentConfig.from_env);
    [cfgAppId, cfgAppSecret, cfgWikiToken].forEach(function (field) {
      field.readOnly = readonly;
    });
    saveButton.disabled = readonly;
    settingsHint.textContent = readonly ? "当前配置来自 .env 文件，请在文件中修改。" : "";
  }

  async function loadConfig() {
    if (!isTauriReady()) {
      return {
        configured: true,
        app_id: "",
        app_secret_hint: "",
        wiki_node_token: "",
        from_env: false,
      };
    }

    return invoke("get_config");
  }

  async function openSettingsPage() {
    try {
      const config = await loadConfig();
      fillSettingsForm(config);
      setSettingsStatus("", null);
      showSettings();
    } catch (error) {
      console.warn("get_config failed", error);
      setSettingsStatus(String(error), "error");
      showSettings();
    }
  }

  function renderMessage(message) {
    const normalized = normalizeMessage(message);
    const existing = messages.get(normalized.id);
    if (existing) {
      updateMessageStatus(normalized.id, normalized.sync_status, normalized.error_reason);
      return;
    }

    const item = createElement("li", "message-item");
    item.dataset.id = normalized.id;

    const bubble = createElement("article", "message-bubble");
    const text = createElement("div", "message-text", normalized.text);
    const errorLine = createElement("div", "message-error");
    const meta = createElement("div", "message-meta");
    const time = createElement("time", "message-time", formatTime(normalized.created_at));
    time.dateTime = normalized.created_at;

    const statusButton = createElement("button", "status-button");
    statusButton.type = "button";
    statusButton.addEventListener("click", function () {
      if (statusButton.dataset.status === "failed") {
        retryMessage(normalized.id);
      }
    });

    meta.append(time, statusButton);
    bubble.append(text, errorLine, meta);
    item.append(bubble);
    messageList.append(item);

    messages.set(normalized.id, {
      data: normalized,
      item,
      statusButton,
      errorLine,
    });

    setStatus(statusButton, normalized.sync_status);
    setErrorText(errorLine, normalized.sync_status, normalized.error_reason);
    scrollToBottom();
  }

  function setStatus(button, status) {
    var key = statusSvg[status] ? status : "queued";
    button.innerHTML = statusSvg[key];
    button.title = statusLabel[key];
    button.setAttribute("aria-label", statusLabel[key]);
    button.dataset.status = status;
    button.disabled = status !== "failed";
  }

  function setErrorText(errorLine, status, reason) {
    if (status === "failed" && reason) {
      errorLine.textContent = reason;
      errorLine.style.display = "";
    } else {
      errorLine.textContent = "";
      errorLine.style.display = "none";
    }
  }

  function updateMessageStatus(id, status, errorReason) {
    const record = messages.get(String(id));
    if (!record) {
      return;
    }

    record.data.sync_status = status;
    record.data.error_reason = errorReason || null;
    setStatus(record.statusButton, status);
    setErrorText(record.errorLine, status, errorReason);
  }

  function scrollToBottom() {
    requestAnimationFrame(function () {
      messageList.scrollTop = messageList.scrollHeight;
    });
  }

  function isMessageListAtBottom() {
    return messageList.scrollHeight - messageList.scrollTop - messageList.clientHeight <= 4;
  }

  function updateAppHeight() {
    var shouldScrollToBottom = messageListWasAtBottom;
    document.documentElement.style.setProperty("--app-height", window.visualViewport.height + "px");
    if (shouldScrollToBottom) {
      scrollToBottom();
    }
  }

  function bindViewportResize() {
    if (!window.visualViewport) {
      return;
    }

    updateAppHeight();
    window.visualViewport.addEventListener("resize", updateAppHeight);
  }

  function resizeInput() {
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 128) + "px";
  }

  async function loadHistory() {
    if (!isTauriReady()) {
      return;
    }

    try {
      const history = await invoke("get_messages", { limit: 50 });
      if (Array.isArray(history)) {
        history.forEach(renderMessage);
      }
    } catch (error) {
      console.warn("get_messages failed", error);
    }
  }

  async function reloadMessages() {
    if (!isTauriReady()) {
      return;
    }

    try {
      const history = await invoke("get_messages", { limit: 50 });
      if (Array.isArray(history)) {
        messages.clear();
        messageList.innerHTML = "";
        history.forEach(renderMessage);
        scrollToBottom();
      }
    } catch (error) {
      console.warn("reloadMessages failed", error);
    }
  }

  async function sendMessage(text) {
    if (!isTauriReady()) {
      renderMessage({
        id: "browser-" + Date.now(),
        text,
        created_at: new Date().toISOString(),
        sync_status: "failed",
      });
      return;
    }

    sendButton.disabled = true;
    try {
      const response = await invoke("send_message", { text });
      if (response && response.status !== "rejected") {
        renderMessage({
          id: response.id,
          text,
          created_at: new Date().toISOString(),
          sync_status: response.status || "queued",
        });
      }
    } catch (error) {
      console.warn("send_message failed", error);
      renderMessage({
        id: "failed-" + Date.now(),
        text,
        created_at: new Date().toISOString(),
        sync_status: "failed",
      });
    } finally {
      sendButton.disabled = false;
      input.focus();
    }
  }

  async function retryMessage(id) {
    const record = messages.get(String(id));
    if (!record || !isTauriReady()) {
      return;
    }

    updateMessageStatus(id, "queued");
    try {
      await invoke("retry_message", { id });
    } catch (error) {
      console.warn("retry_message failed", error);
      updateMessageStatus(id, "failed");
    }
  }

  async function bindSyncEvents() {
    if (typeof listen !== "function") {
      return;
    }

    try {
      await listen("sync_status_changed", function (event) {
        const payload = event && event.payload ? event.payload : {};
        if (payload.id && payload.status) {
          updateMessageStatus(payload.id, payload.status, payload.error);
        }
      });
      await listen("messages_updated", function () {
        reloadMessages();
      });
    } catch (error) {
      console.warn("event listener setup failed", error);
    }
  }

  async function saveSettings() {
    if (!isTauriReady()) {
      showChat();
      return;
    }

    saveButton.disabled = true;
    testButton.disabled = true;
    setSettingsStatus("正在保存...", null);
    try {
      const config = await invoke("save_config", {
        appId: cfgAppId.value,
        appSecret: cfgAppSecret.value,
        wikiNodeToken: cfgWikiToken.value,
      });
      fillSettingsForm(config);
      setSettingsStatus("已保存", "success");
      showChat();
    } catch (error) {
      console.warn("save_config failed", error);
      setSettingsStatus(String(error), "error");
    } finally {
      saveButton.disabled = Boolean(currentConfig && currentConfig.from_env);
      testButton.disabled = false;
    }
  }

  async function testSettings() {
    if (!isTauriReady()) {
      setSettingsStatus("浏览器预览模式无法测试连接", "error");
      return;
    }

    testButton.disabled = true;
    setSettingsStatus("正在测试连接...", null);
    try {
      const result = await invoke("test_connection", {
        appId: cfgAppId.value || null,
        appSecret: cfgAppSecret.value || null,
        wikiNodeToken: cfgWikiToken.value || null,
      });
      if (result && result.success) {
        setSettingsStatus("连接成功", "success");
      } else {
        const error = result && result.error ? result.error : "连接失败";
        setSettingsStatus(error, "error");
      }
    } catch (error) {
      console.warn("test_connection failed", error);
      setSettingsStatus(String(error), "error");
    } finally {
      testButton.disabled = false;
    }
  }

  composer.addEventListener("submit", function (event) {
    event.preventDefault();
    const text = input.value.trim();
    if (!text) {
      return;
    }

    input.value = "";
    resizeInput();
    sendMessage(text);
  });

  openSettings.addEventListener("click", openSettingsPage);

  settingsBack.addEventListener("click", function () {
    if (currentConfig && currentConfig.configured) {
      showChat();
    } else {
      setSettingsStatus("请先保存有效配置", "error");
    }
  });

  settingsForm.addEventListener("submit", function (event) {
    event.preventDefault();
    saveSettings();
  });

  testButton.addEventListener("click", testSettings);

  messageList.addEventListener("scroll", function () {
    messageListWasAtBottom = isMessageListAtBottom();
  });

  input.addEventListener("input", resizeInput);
  input.addEventListener("keydown", function (event) {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      composer.requestSubmit();
    }
  });

  var loading = document.getElementById("loading");
  function hideLoading() {
    if (loading) {
      loading.remove();
      loading = null;
    }
  }

  var exportButton = document.getElementById("btn-export-logs");

  async function exportLogs() {
    if (!isTauriReady()) {
      setSettingsStatus("浏览器预览模式无法导出", "error");
      return;
    }

    exportButton.disabled = true;
    setSettingsStatus("正在生成日志...", null);
    try {
      var text = await invoke("export_logs");
      try {
        await navigator.clipboard.writeText(text);
        setSettingsStatus("日志已复制到剪贴板，可粘贴发送", "success");
      } catch (_) {
        prompt("复制以下日志内容：", text);
        setSettingsStatus("请手动复制日志内容", null);
      }
    } catch (error) {
      console.warn("export_logs failed", error);
      setSettingsStatus(String(error), "error");
    } finally {
      exportButton.disabled = false;
    }
  }

  if (exportButton) {
    exportButton.addEventListener("click", exportLogs);
  }

  resizeInput();
  bindViewportResize();
  bindSyncEvents();
  loadConfig()
    .then(function (config) {
      fillSettingsForm(config);
      hideLoading();
      if (config && config.configured) {
        showChat();
      } else {
        showSettings();
      }
    })
    .catch(function (error) {
      console.warn("initial get_config failed", error);
      hideLoading();
      showSettings();
      setSettingsStatus(String(error), "error");
    });
})();
