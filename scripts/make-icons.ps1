# assets/icon/icon.svg から Windows 用の icon.ico と macOS 用の icon.icns を作る。
# Windows 専用。SVG の描画には Microsoft Edge のヘッドレス表示を、縮小には System.Drawing を使う。
# 使い方: pwsh scripts/make-icons.ps1
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$root = Split-Path $PSScriptRoot
$iconDir = Join-Path $root 'assets/icon'
$svg = Join-Path $iconDir 'icon.svg'
$work = Join-Path ([IO.Path]::GetTempPath()) "hatebu-reader-icons-$PID"
New-Item -ItemType Directory -Force $work | Out-Null

$edge = @(
    "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe",
    "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $edge) { throw 'Microsoft Edge が見つかりません' }

# 1024px で描く。$inset は余白（macOS のアイコンは周囲に余白を取る）。
function Render-Master([string]$name, [int]$inset) {
    $size = 1024 - 2 * $inset
    $html = Join-Path $work "$name.html"
    $png = Join-Path $work "$name.png"
    $src = ([Uri]$svg).AbsoluteUri
    Set-Content -Encoding utf8 $html "<!doctype html><html><body style='margin:0;background:transparent'><img src='$src' style='position:fixed;left:${inset}px;top:${inset}px;width:${size}px;height:${size}px'></body></html>"
    $profileDir = Join-Path $work "profile-$name"
    & $edge --headless --disable-gpu --hide-scrollbars --no-first-run "--user-data-dir=$profileDir" `
        --default-background-color=00000000 --force-device-scale-factor=1 --window-size=1024,1024 `
        "--screenshot=$png" ([Uri]$html).AbsoluteUri | Out-Null
    # msedge.exe はすぐ戻り、画像は後から書き出される。
    for ($i = 0; $i -lt 100 -and -not (Test-Path $png); $i++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 500
    if (-not (Test-Path $png)) { throw "描画に失敗しました: $name" }
    $png
}

function Resize-Png([string]$master, [int]$size) {
    $src = [Drawing.Bitmap]::FromFile($master)
    try {
        $dst = New-Object Drawing.Bitmap $size, $size, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $g = [Drawing.Graphics]::FromImage($dst)
        $g.CompositingMode = [Drawing.Drawing2D.CompositingMode]::SourceCopy
        $g.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $g.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::HighQuality
        $g.DrawImage($src, 0, 0, $size, $size)
        $g.Dispose()
        $ms = New-Object IO.MemoryStream
        $dst.Save($ms, [Drawing.Imaging.ImageFormat]::Png)
        $dst.Dispose()
        , $ms.ToArray()
    } finally { $src.Dispose() }
}

function Write-BigEndian([IO.BinaryWriter]$w, [uint32]$value) {
    $bytes = [BitConverter]::GetBytes($value)
    [Array]::Reverse($bytes)
    $w.Write($bytes)
}

try {
    $windows = Render-Master 'windows' 0
    $mac = Render-Master 'mac' 100

    # .ico: 各サイズを PNG のまま格納する（Windows Vista 以降が対応）。
    $sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
    $images = foreach ($s in $sizes) { , (Resize-Png $windows $s) }
    $ico = New-Object IO.BinaryWriter ([IO.File]::Create((Join-Path $iconDir 'icon.ico')))
    try {
        $ico.Write([uint16]0); $ico.Write([uint16]1); $ico.Write([uint16]$sizes.Count)
        $offset = 6 + 16 * $sizes.Count
        for ($i = 0; $i -lt $sizes.Count; $i++) {
            $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
            $ico.Write([byte]$dim); $ico.Write([byte]$dim); $ico.Write([byte]0); $ico.Write([byte]0)
            $ico.Write([uint16]1); $ico.Write([uint16]32)
            $ico.Write([uint32]$images[$i].Length); $ico.Write([uint32]$offset)
            $offset += $images[$i].Length
        }
        foreach ($image in $images) { $ico.Write($image) }
    } finally { $ico.Dispose() }

    # .icns: 種類ごとの PNG を並べる。
    $types = [ordered]@{ ic11 = 32; ic12 = 64; ic07 = 128; ic13 = 256; ic08 = 256; ic14 = 512; ic09 = 512; ic10 = 1024 }
    $entries = foreach ($type in $types.Keys) { [pscustomobject]@{ Type = $type; Data = (Resize-Png $mac $types[$type]) } }
    $total = 8 + ($entries | ForEach-Object { 8 + $_.Data.Length } | Measure-Object -Sum).Sum
    $icns = New-Object IO.BinaryWriter ([IO.File]::Create((Join-Path $iconDir 'icon.icns')))
    try {
        $icns.Write([Text.Encoding]::ASCII.GetBytes('icns')); Write-BigEndian $icns $total
        foreach ($entry in $entries) {
            $icns.Write([Text.Encoding]::ASCII.GetBytes($entry.Type))
            Write-BigEndian $icns (8 + $entry.Data.Length)
            $icns.Write($entry.Data)
        }
    } finally { $icns.Dispose() }

    # 見た目の確認用。
    [IO.File]::WriteAllBytes((Join-Path $iconDir 'icon-256.png'), (Resize-Png $windows 256))
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
