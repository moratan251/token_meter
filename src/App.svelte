<script lang="ts">
  import { onMount } from "svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import Dashboard from "./pages/Dashboard.svelte";
  import Explorer from "./pages/Explorer.svelte";
  import Subscription from "./pages/Subscription.svelte";
  import Limits from "./pages/Limits.svelte";
  import Pricing from "./pages/Pricing.svelte";
  import SettingsPage from "./pages/Settings.svelte";
  import { api, fetchLitellm, fetchUsdJpy, onScanProgress } from "./lib/api";
  import { app, loadMeta, refreshPeriod, saveSettings, scan, toast } from "./lib/state.svelte";
  import { theme } from "./lib/colors.svelte";

  const DAY = 86_400_000;
  let ready = $state(false);

  // テーマ: 明示的な設定を優先し、"system" のときは OS の設定に追従する。
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  let osDark = $state(media.matches);
  media.addEventListener("change", (e) => (osDark = e.matches));
  $effect(() => {
    const t = app.settings?.theme ?? "system";
    const dark = t === "dark" || (t === "system" && osDark);
    theme.dark = dark;
    document.documentElement.dataset.theme = dark ? "dark" : "light";
  });

  // 自動更新: ログを読み直し、相対的な期間を現在に合わせる。
  $effect(() => {
    const secs = app.settings?.auto_refresh_secs ?? 0;
    if (!ready || secs <= 0) return;
    const t = setInterval(() => {
      refreshPeriod();
      scan();
    }, secs * 1000);
    return () => clearInterval(t);
  });

  async function onlineUpdates() {
    const s = app.settings;
    if (!s?.auto_update_prices) return;
    const fetchedAt = app.meta?.litellm_fetched_at ? Date.parse(app.meta.litellm_fetched_at) : 0;
    if (Date.now() - fetchedAt > 3 * DAY) {
      try {
        await fetchLitellm();
        await loadMeta();
        app.dataVersion++;
      } catch (e) {
        console.warn(e);
      }
    }
    const fxAt = s.usd_jpy_updated ? Date.parse(s.usd_jpy_updated) : 0;
    if (Date.now() - fxAt > DAY) {
      try {
        const rate = await fetchUsdJpy();
        await saveSettings({ ...app.settings!, usd_jpy: rate, usd_jpy_updated: new Date().toISOString() });
      } catch (e) {
        console.warn(e);
      }
    }
  }

  onMount(() => {
    const unlisten = onScanProgress((p) => {
      app.scanProgress = p;
    });
    (async () => {
      try {
        app.settings = await api.getSettings();
        await scan();
        ready = true;
        onlineUpdates();
      } catch (e) {
        app.error = String(e);
        toast(String(e), "error");
      }
    })();
    return () => {
      unlisten.then((f) => f());
    };
  });
</script>

<div class="shell">
  <Sidebar />
  <main>
    {#if !ready}
      <div class="loading">
        <div class="spinner"></div>
        <div>ログを読み込んでいます…</div>
        {#if app.scanProgress && app.scanProgress.total > 0}
          <div class="muted num">{app.scanProgress.done} / {app.scanProgress.total} ファイル</div>
        {/if}
        {#if app.error}<div class="banner">{app.error}</div>{/if}
      </div>
    {:else if app.page === "dashboard"}
      <Dashboard />
    {:else if app.page === "explorer"}
      <Explorer />
    {:else if app.page === "subscription"}
      <Subscription />
    {:else if app.page === "limits"}
      <Limits />
    {:else if app.page === "pricing"}
      <Pricing />
    {:else}
      <SettingsPage />
    {/if}
  </main>
</div>

{#if app.toast}
  <div class="toast" class:error={app.toast.kind === "error"} role="status">{app.toast.text}</div>
{/if}

<style>
  .shell {
    display: flex;
    height: 100%;
  }
  main {
    flex: 1;
    min-width: 0;
    overflow: auto;
    position: relative;
  }
  .loading {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    color: var(--text-2);
  }
  .spinner {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    border: 3px solid var(--border-strong);
    border-top-color: var(--accent);
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .toast {
    position: fixed;
    right: 20px;
    bottom: 20px;
    max-width: 480px;
    background: var(--surface);
    border: 1px solid var(--border-strong);
    border-left: 4px solid var(--accent);
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.18);
    border-radius: 8px;
    padding: 10px 14px;
    z-index: 100;
  }
  .toast.error {
    border-left-color: var(--critical);
  }
</style>
