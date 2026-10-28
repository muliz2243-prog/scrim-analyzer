# Sonda RE: ¿las muertes se detectan en el stream de posiciones validado?
# Cruza huecos/quietud contra los 51 timestamps de kill y las muertes por campeón.
$j = Get-Content 'salida_moderno.json' -Raw | ConvertFrom-Json

# Muertes por slot (statsJson, verificado contra totales de equipo).
$deaths = @{0=9;1=4;2=5;3=6;4=8;5=1;6=6;7=6;8=3;9=3}
$kills = @($j.events | Where-Object { $_.kind.type -eq 'champion_kill' } | ForEach-Object { $_.timestamp_ms })
Write-Output "kills=$($kills.Count) muertes_totales=$(($deaths.Values | Measure-Object -Sum).Sum)"

# Histograma de dt entre muestras consecutivas (todas, por campeón).
$hist = @{}
foreach ($p in $j.positions) {
    $s = $p.samples
    for ($i = 1; $i -lt $s.Count; $i++) {
        $dt = $s[$i].t_ms - $s[$i-1].t_ms
        $hist[$dt] = [int]$hist[$dt] + 1
    }
}
Write-Output '== histograma dt (ms: n):'
$hist.GetEnumerator() | Sort-Object Name | Select-Object -First 15 | ForEach-Object { "  $($_.Key): $($_.Value)" }

# H1: huecos (dt grande) = muerte. Varios umbrales.
Write-Output '== H1 huecos por umbral de dt:'
foreach ($thr in @(3000, 4000, 5000, 6000, 8000)) {
    $runs = @()
    $per = New-Object 'System.Collections.Generic.SortedDictionary[int,int]'
    foreach ($p in $j.positions) {
        $s = $p.samples; $cnt = 0
        for ($i = 1; $i -lt $s.Count; $i++) {
            $dt = $s[$i].t_ms - $s[$i-1].t_ms
            if ($dt -ge $thr) {
                $runs += ,([pscustomobject]@{ slot = $p.slot; team = $p.team; start = $s[$i-1].t_ms; end = $s[$i].t_ms })
                $cnt++
            }
        }
        $per[[int]$p.slot] = $cnt
    }
    $matched = 0
    foreach ($k in $kills) {
        foreach ($r in $runs) { if ($r.start -le $k -and $r.end -ge $k) { $matched++; break } }
    }
    $perStr = ($per.GetEnumerator() | ForEach-Object { $_.Value }) -join ','
    $expStr = ($deaths.GetEnumerator() | Sort-Object Key | ForEach-Object { $_.Value }) -join ','
    Write-Output "  thr=${thr}ms runs=$($runs.Count) kills_cubiertas=$matched/51"
    Write-Output "        por_slot: [$perStr]  esperado(muertes): [$expStr]"
}

# H2: quieto (coordenadas idénticas sostenidas) = muerte.
Write-Output '== H2 rachas quietas por umbral de duración:'
foreach ($thr in @(8000, 10000, 12000)) {
    $runs = @()
    $per = @{}
    foreach ($p in $j.positions) {
        $s = $p.samples; $cnt = 0
        $i = 0
        while ($i -lt $s.Count) {
            $jx = $i
            while ($jx + 1 -lt $s.Count -and $s[$jx+1].x -eq $s[$i].x -and $s[$jx+1].y -eq $s[$i].y) { $jx++ }
            $dur = $s[$jx].t_ms - $s[$i].t_ms
            if ($dur -ge $thr) { $runs += ,([pscustomobject]@{ slot = $p.slot; team = $p.team; start = $s[$i].t_ms; end = $s[$jx].t_ms }); $cnt++ }
            $i = $jx + 1
        }
        $per[[int]$p.slot] = $cnt
    }
    $matched = 0
    foreach ($k in $kills) {
        foreach ($r in $runs) { if ($r.start -le $k -and $r.end -ge $k) { $matched++; break } }
    }
    $perStr = ($per.GetEnumerator() | Sort-Object Key | ForEach-Object { $_.Value }) -join ','
    Write-Output "  thr=${thr}ms rachas=$($runs.Count) kills_cubiertas=$matched/51"
    Write-Output "        por_slot: [$perStr]"
}
