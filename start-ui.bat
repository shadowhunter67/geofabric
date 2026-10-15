@echo off
rem GeoFabric - start the web UI (gateway + built UI) and open it in the browser.
rem Usage:  start-ui.bat            normal start
rem         start-ui.bat rebuild    rebuild the Rust gateway and the web UI first
rem         start-ui.bat nobrowser  start the server only
rem Needs: Rust, Node, Python on PATH (first run only: they are used to prepare things).
setlocal EnableExtensions
chcp 65001 >nul
pushd "%~dp0"

set PORT=8787
set REBUILD=0
set OPEN=1
for %%A in (%*) do (
  if /I "%%A"=="rebuild" set REBUILD=1
  if /I "%%A"=="nobrowser" set OPEN=0
)

rem ---- 1. prebuilt DuckDB library (verified download, see ADR-006) -------------------------
set "DUCKDB_LIB_DIR=%CD%\tools\.cache\duckdb-1.5.6\lib"
if not exist "%DUCKDB_LIB_DIR%\duckdb.dll" (
  echo [1/4] Downloading and verifying the DuckDB library ^(first run only^)...
  python tools\duckdb_prebuilt.py || goto :fail
)
set "PATH=%DUCKDB_LIB_DIR%;%PATH%"

rem ---- 2. data ------------------------------------------------------------------------------
dir /b "benchmarks\data\*.parquet" >nul 2>&1
if errorlevel 1 (
  echo [!] No GeoParquet files in benchmarks\data.
  echo     Create sample data first, for example:
  echo       python -m venv benchmarks\.venv
  echo       benchmarks\.venv\Scripts\python -m pip install pyarrow duckdb numpy
  echo       benchmarks\.venv\Scripts\python benchmarks\gen_synthetic.py --size 10mb --out benchmarks\data
  goto :fail
)

rem ---- stale check: rebuild automatically when sources are newer than the built outputs ----------
powershell -NoProfile -Command "$o=(Get-Item 'web\dist\index.html' -ErrorAction SilentlyContinue).LastWriteTime; if(-not $o -or (Get-ChildItem web\src,web\index.html,web\public -Recurse -File | Where-Object {$_.LastWriteTime -gt $o})){exit 1}"
if errorlevel 1 set WEB_STALE=1
powershell -NoProfile -Command "$o=(Get-Item 'target\release\geo-gateway.exe' -ErrorAction SilentlyContinue).LastWriteTime; if(-not $o -or (Get-ChildItem crates -Recurse -File -Include *.rs,Cargo.toml | Where-Object {$_.LastWriteTime -gt $o})){exit 1}"
if errorlevel 1 set GW_STALE=1

rem ---- 3. web UI bundle ---------------------------------------------------------------------
if "%REBUILD%"=="1" goto :build_web
if "%WEB_STALE%"=="1" goto :build_web
if not exist "web\dist\index.html" goto :build_web
goto :web_ok
:build_web
echo [2/4] Building the web UI...
pushd web
if not exist node_modules call npm install || (popd & goto :fail)
call npm run build || (popd & goto :fail)
popd
:web_ok

rem ---- 4. gateway (release build, prebuilt DuckDB) --------------------------------------------
if "%REBUILD%"=="1" goto :build_gw
if "%GW_STALE%"=="1" goto :build_gw
if not exist "target\release\geo-gateway.exe" goto :build_gw
goto :gw_ok
:build_gw
echo [3/4] Building the gateway ^(release; a few minutes the first time^)...
set CARGO_BUILD_JOBS=4
cargo build --release -p geo-gateway --no-default-features || goto :fail
:gw_ok

rem ---- already running? ---------------------------------------------------------------------
curl -s -m 2 http://127.0.0.1:%PORT%/api/health >nul 2>&1
if not errorlevel 1 (
  echo GeoFabric is already running on port %PORT%.
  echo If the page looks old: close the "GeoFabric gateway" window and run start-ui.bat again.
  goto :open
)

echo [4/4] Starting the gateway on http://127.0.0.1:%PORT% ...
start "GeoFabric gateway (close this window to stop)" /D "%CD%" cmd /k "set PATH=%DUCKDB_LIB_DIR%;%PATH% && target\release\geo-gateway.exe --data benchmarks\data --web web\dist --port %PORT%"

set /a TRIES=0
:wait
curl -s -m 2 http://127.0.0.1:%PORT%/api/health >nul 2>&1
if not errorlevel 1 goto :open
set /a TRIES+=1
if %TRIES% GEQ 30 (
  echo The gateway did not answer within 30 seconds - see its window for the error.
  goto :fail
)
timeout /t 1 /nobreak >nul
goto :wait

:open
if "%OPEN%"=="1" start "" "http://127.0.0.1:%PORT%/"
echo.
echo GeoFabric UI: http://127.0.0.1:%PORT%/   ^(close the "GeoFabric gateway" window to stop it^)
popd
endlocal
exit /b 0

:fail
echo.
echo Start-up failed. Fix the message above and run start-ui.bat again.
popd
pause
exit /b 1
