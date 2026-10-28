# Desglose de cada ventana con alerta: kills y torres deduplicados por ventana.
$cap = Get-Content 'validacion_scrim.json' -Raw | ConvertFrom-Json

$kills = @($cap.events | Where-Object kind -eq 'ChampionKill' | Sort-Object t_game_s)
$kept = New-Object System.Collections.ArrayList
foreach ($e in $kills) {
    $dup = $false
    foreach ($k in $kept) {
        if ($k.killer -eq $e.killer -and $k.victim -eq $e.victim -and [math]::Abs($e.t_game_s - $k.t_game_s) -le 5.0) { $dup = $true; break }
    }
    if (-not $dup) { [void]$kept.Add($e) }
}
$turrets = @($cap.events | Where-Object kind -eq 'TurretKilled' | Sort-Object t_game_s)
$tk = New-Object System.Collections.ArrayList
foreach ($e in $turrets) {
    $dup = $false
    foreach ($k in $tk) {
        if ($k.killer -eq $e.killer -and $k.turret -eq $e.turret -and [math]::Abs($e.t_game_s - $k.t_game_s) -le 5.0) { $dup = $true; break }
    }
    if (-not $dup) { [void]$tk.Add($e) }
}
"torres: crudas=$($turrets.Count) dedup=$($tk.Count)"

function MmSs([double]$s) {
    $t = [int][math]::Floor($s)
    $mm = [int][math]::Floor($t / 60)
    $ss = [int]($t % 60)
    return "{0:D2}:{1:D2}" -f $mm, $ss
}

# Ventanas con alerta (del recalc).
$windows = @(@(180,210),@(360,390),@(390,420),@(540,570),@(600,630),@(720,750),@(900,930),@(1020,1050),@(1230,1260),@(1290,1320),@(1320,1350))
foreach ($w in $windows) {
    $s = $w[0]; $e2 = $w[1]
    Write-Output ("[{0}-{1}]" -f (MmSs $s), (MmSs $e2))
    foreach ($k in $kept) {
        if ($k.t_game_s -ge $s -and $k.t_game_s -lt $e2) {
            $kt = if ($k.team -eq 'blue') { 'AZUL' } else { 'ROJO' }
            $vt = if ($k.victim_team -eq 'blue') { 'AZUL' } else { 'ROJO' }
            Write-Output ("    {0} kill {1}: {2} -> {3} ({4})" -f (MmSs $k.t_game_s), $kt, $k.killer, $k.victim, $vt)
        }
    }
    foreach ($t in $tk) {
        if ($t.t_game_s -ge $s -and $t.t_game_s -lt $e2) {
            $tt = if ($t.team -eq 'blue') { 'AZUL' } else { 'ROJO' }
            Write-Output ("    {0} TORRE destruida por {1}: {2}" -f (MmSs $t.t_game_s), $tt, $t.turret)
        }
    }
}
