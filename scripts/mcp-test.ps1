Param(
    [switch]$NoUi
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Join-Path $Root ".."
Set-Location $Root

Write-Host "[mcp-test] starting dev MCP test server..."
$server = Start-Process -FilePath "cargo" -ArgumentList "mcp-test" -PassThru -WindowStyle Hidden -RedirectStandardOutput "$env:TEMP\mcp-test-server.log" -RedirectStandardError "$env:TEMP\mcp-test-server.log"
Write-Host "[mcp-test] server pid $($server.Id) (logs: $env:TEMP\mcp-test-server.log)"

try {
    Write-Host "[mcp-test] launching app with PLUGABLE_ENABLE_MCP_TEST=1"
    $env:PLUGABLE_ENABLE_MCP_TEST = "1"
    npx tauri dev
}
finally {
    if ($server -and !$server.HasExited) {
        Write-Host "[mcp-test] stopping server pid $($server.Id)"
        $server.Kill()
    }
}
