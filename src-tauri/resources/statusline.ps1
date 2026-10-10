# Token Meter: Claude Code のステータスラインから使用上限（%）を記録するスクリプト。
#
# Claude Code はステータスラインのコマンドに、標準入力で JSON を渡す。Pro / Max では
# rate_limits.five_hour / seven_day に使用率（used_percentage、0〜100）とリセット時刻
# （resets_at、Unix 秒）が入るので、値が変わったときだけ同じフォルダの
# rate-limits.jsonl に 1 行追記する。Token Meter はこのファイルを読み込む。
#
# Token Meter の「使用上限との比較」画面から設定・解除できる。手で消す場合は
# ~/.claude/settings.json の statusLine を削除する。

$ErrorActionPreference = 'SilentlyContinue'
[Console]::InputEncoding = [System.Text.Encoding]::UTF8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$raw = [Console]::In.ReadToEnd()
$j = $null
try { $j = $raw | ConvertFrom-Json } catch { }

$parts = @()
if ($j -and $j.model -and $j.model.display_name) { $parts += [string]$j.model.display_name }

$rl = $null
if ($j) { $rl = $j.rate_limits }
if ($rl) {
    $inv = [System.Globalization.CultureInfo]::InvariantCulture
    $fields = @('"t":' + [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())
    $key = ''
    foreach ($w in @(@('five_hour', 'fh', '5h'), @('seven_day', 'sd', '7d'))) {
        $v = $rl.($w[0])
        if ($v -and $null -ne $v.used_percentage) {
            $p = [double]$v.used_percentage
            $fields += ('"' + $w[1] + '":' + $p.ToString($inv))
            if ($null -ne $v.resets_at) {
                $r = [long][double]$v.resets_at
                $fields += ('"' + $w[1] + '_reset":' + $r)
                $key += "$($w[1])=$p@$r;"
            } else {
                $key += "$($w[1])=$p;"
            }
            $parts += ($w[2] + ' ' + [math]::Round($p).ToString($inv) + '%')
        }
    }
    if ($key -ne '') {
        # 並行するセッションが同じ値を何度も書かないよう、前回書いた値と比べる。
        $state = Join-Path $PSScriptRoot 'last.txt'
        $prev = ''
        if (Test-Path $state) { $prev = [System.IO.File]::ReadAllText($state) }
        if ($prev -ne $key) {
            $line = '{' + ($fields -join ',') + '}'
            [System.IO.File]::AppendAllText((Join-Path $PSScriptRoot 'rate-limits.jsonl'), $line + "`n")
            [System.IO.File]::WriteAllText($state, $key)
        }
    }
}

[Console]::Out.Write(($parts -join ' | '))
