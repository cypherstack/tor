# Point cargo, target, and temp directories at short paths on Windows runners.
$ErrorActionPreference = 'Stop'
git config --global core.longpaths true
New-Item -ItemType Directory -Force C:\c, C:\t, C:\tmp | Out-Null
"CARGO_HOME=C:/c" >> $env:GITHUB_ENV
"CARGO_TARGET_DIR=C:/t" >> $env:GITHUB_ENV
"TEMP=C:\tmp" >> $env:GITHUB_ENV
"TMP=C:\tmp" >> $env:GITHUB_ENV
