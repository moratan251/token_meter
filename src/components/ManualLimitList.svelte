<script lang="ts">
  import { app, saveSettings, toast } from "../lib/state.svelte";
  import { SERVICE_LABEL, dateTime, pad } from "../lib/format";
  import type { LimitKind, ManualLimit, ServiceId } from "../lib/types";

  let {
    records,
    showKind = false,
  }: {
    /** 表示する手動記録（新しい順に並べ替えて表示する） */
    records: ManualLimit[];
    /** サービス・枠の列を出す（全件一覧用） */
    showKind?: boolean;
  } = $props();

  type Draft = {
    service: ServiceId;
    kind: LimitKind;
    model: string;
    percent: number | null;
    ts: string;
    resets: string;
  };

  let editing = $state<string | null>(null);
  let draft = $state<Draft | null>(null);

  const sorted = $derived([...records].sort((a, b) => b.ts - a.ts));

  /** datetime-local 入力用の "YYYY-MM-DDTHH:MM"（ローカル時刻） */
  function toInput(ts: number | null): string {
    if (ts == null) return "";
    const d = new Date(ts);
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }

  function fromInput(s: string): number | null {
    if (!s) return null;
    const t = new Date(s).getTime();
    return isFinite(t) ? t : null;
  }

  function kindLabel(kind: LimitKind, model: string | null): string {
    if (kind === "session") return "5時間枠";
    if (kind === "weekly") return "週間（すべて）";
    return `週間（${model ?? "モデル別"}）`;
  }

  function startEdit(x: ManualLimit) {
    editing = x.id;
    draft = {
      service: x.service,
      kind: x.kind,
      model: x.model ?? "",
      percent: x.percent,
      ts: toInput(x.ts),
      resets: toInput(x.resets_at),
    };
  }

  function cancel() {
    editing = null;
    draft = null;
  }

  async function save(id: string) {
    if (!app.settings || !draft) return;
    const ts = fromInput(draft.ts);
    const resets = fromInput(draft.resets);
    if (draft.percent == null || !(draft.percent >= 0 && draft.percent <= 100)) {
      toast("使用率は 0〜100 で入力してください", "error");
      return;
    }
    if (ts == null) {
      toast("記録日時を入力してください", "error");
      return;
    }
    if (resets != null && resets <= ts) {
      toast("リセット日時は記録日時より後にしてください", "error");
      return;
    }
    if (draft.kind === "weekly_model" && !draft.model.trim()) {
      toast("モデル別の枠にはモデル名（例: fable）を入力してください", "error");
      return;
    }
    const d = draft;
    const manual_limits = app.settings.manual_limits.map((x) =>
      x.id === id
        ? {
            ...x,
            service: d.service,
            kind: d.kind,
            model: d.kind === "weekly_model" ? d.model.trim().toLowerCase() : null,
            percent: Number(d.percent),
            ts,
            resets_at: resets,
          }
        : x,
    );
    await saveSettings({ ...app.settings, manual_limits });
    cancel();
    toast("記録を更新しました");
  }

  async function remove(x: ManualLimit) {
    if (!app.settings) return;
    if (!confirm(`${dateTime(x.ts)} の記録（${x.percent}%）を削除しますか？`)) return;
    await saveSettings({ ...app.settings, manual_limits: app.settings.manual_limits.filter((r) => r.id !== x.id) });
    if (editing === x.id) cancel();
  }
</script>

<table class="data list">
  <thead>
    <tr>
      <th>記録日時</th>
      {#if showKind}<th>サービス</th><th>枠</th>{/if}
      <th class="r">使用率</th>
      <th>リセット日時</th>
      <th></th>
    </tr>
  </thead>
  <tbody>
    {#each sorted as x (x.id)}
      {#if editing === x.id && draft}
        <tr class="edit">
          <td><input class="input" type="datetime-local" bind:value={draft.ts} aria-label="記録日時" /></td>
          {#if showKind}
            <td>
              <select class="input" bind:value={draft.service} aria-label="サービス">
                <option value="claude">Claude</option>
                <option value="codex">Codex</option>
              </select>
            </td>
            <td class="nowrap">
              <select class="input" bind:value={draft.kind} aria-label="枠">
                <option value="session">5時間枠</option>
                <option value="weekly">週間（すべて）</option>
                <option value="weekly_model">週間（モデル別）</option>
              </select>
              {#if draft.kind === "weekly_model"}
                <input class="input" bind:value={draft.model} placeholder="fable" style="width:90px" aria-label="モデル名" />
              {/if}
            </td>
          {/if}
          <td class="r nowrap">
            <input class="input" type="number" min="0" max="100" step="any" bind:value={draft.percent} style="width:76px" aria-label="使用率" /> %
          </td>
          <td class="nowrap">
            <input class="input" type="datetime-local" bind:value={draft.resets} aria-label="リセット日時" />
            {#if draft.resets}
              <button class="btn small ghost" onclick={() => draft && (draft.resets = "")} title="リセット日時を消す">消す</button>
            {/if}
          </td>
          <td class="r nowrap">
            <button class="btn small primary" onclick={() => save(x.id)}>保存</button>
            <button class="btn small" onclick={cancel}>キャンセル</button>
          </td>
        </tr>
      {:else}
        <tr>
          <td>{dateTime(x.ts)}</td>
          {#if showKind}
            <td>{SERVICE_LABEL[x.service]}</td>
            <td>{kindLabel(x.kind, x.model)}</td>
          {/if}
          <td class="r">{x.percent}%</td>
          <td>{x.resets_at ? dateTime(x.resets_at) : "—"}</td>
          <td class="r nowrap">
            <button class="btn small ghost" onclick={() => startEdit(x)}>編集</button>
            <button class="btn small ghost danger" onclick={() => remove(x)}>削除</button>
          </td>
        </tr>
      {/if}
    {:else}
      <tr><td colspan={showKind ? 6 : 4} class="muted">記録はありません</td></tr>
    {/each}
  </tbody>
</table>

<style>
  .list {
    margin-top: 6px;
  }
  .nowrap {
    white-space: nowrap;
  }
  tr.edit td {
    background: var(--surface-2);
  }
  tr.edit:hover td {
    background: var(--surface-2);
  }
</style>
