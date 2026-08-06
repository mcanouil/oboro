# @license MIT
# @copyright 2026 Mickaël Canouil
# @author Mickaël Canouil
#
# oboro installer.
#
#   powershell -ExecutionPolicy ByPass -c "irm https://m.canouil.dev/oboro/install.ps1 | iex"
#
# Downloads the prebuilt release binary for this machine, verifies it against
# the release SHA256SUMS, and installs it. The default binary reads txt, md,
# docx, xlsx and pdf, and touches no network. No prebuilt Windows build offers
# name recognition (ner) or image reading (ocr) yet; both need a source build,
# with `cargo build --release --features ner` or `--features ocr`.
#
# Environment variables:
#   OBORO_VERSION             Install this version instead of the latest.
#   OBORO_INSTALL_DIR         Install here instead of the resolved default.
#   OBORO_FEATURES            Accepted for parity with install.sh; any value
#                              other than an empty string is refused, since no
#                              prebuilt Windows feature build is published.
#   OBORO_SKIP_CHECKSUM=1     Skip SHA256 verification (not recommended).
#   OBORO_VERIFY_PROVENANCE=1 Also verify build provenance with the gh CLI.
#
# irm | iex cannot take arguments; run the parameterised form instead:
#
#   & ([scriptblock]::Create((irm https://m.canouil.dev/oboro/install.ps1))) -Version 0.7.0

#Requires -Version 5.1

param(
	[string]$Version = $env:OBORO_VERSION,
	[string]$Dir = $env:OBORO_INSTALL_DIR,
	[string]$Features = $env:OBORO_FEATURES,
	[switch]$Help
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1's default SecurityProtocol excludes TLS 1.2 on an
# unpatched machine; PowerShell 7+ already defaults to it.
if ($PSVersionTable.PSEdition -eq 'Desktop' -or $PSVersionTable.PSVersion.Major -lt 6) {
	[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
}

$Repo = 'mcanouil/oboro'
$BinaryName = 'oboro'

# Every writer below suppresses PSAvoidUsingWriteHost for the same reason:
# installer output belongs on the console with colour, mirroring install.sh,
# where Write-Output would put it on the pipeline instead.
#
# Suppressed per function rather than once ahead of param(). An attribute
# between #Requires and param() is legal in a file, but not in a script parsed
# from a string, which is exactly how `irm | iex` and [scriptblock]::Create
# parse this one: both then refuse the whole script.
function Write-Info {
	[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
		Justification = 'Installer output belongs on the console with colour.')]
	param([string]$Message)
	Write-Host $Message -ForegroundColor Green
}

function Write-Warn {
	[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
		Justification = 'Installer output belongs on the console with colour.')]
	param([string]$Message)
	Write-Host $Message -ForegroundColor Yellow
}

# Throws rather than calling exit: this script runs inside the caller's own
# session, by design, either piped into iex or invoked as a scriptblock, and
# exit there would close that session rather than just stopping the install.
function Write-Fail {
	[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
		Justification = 'Installer output belongs on the console with colour.')]
	param([string]$Message)
	Write-Host $Message -ForegroundColor Red
	throw $Message
}

function Show-Usage {
	[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
		Justification = 'Installer output belongs on the console with colour.')]
	param()
	@"
oboro installer

Usage:
  powershell -ExecutionPolicy ByPass -c "irm https://m.canouil.dev/oboro/install.ps1 | iex"
  & ([scriptblock]::Create((irm https://m.canouil.dev/oboro/install.ps1))) [-Version <version>] [-Dir <path>] [-Help]

Options:
  -Version <version>  Install this version instead of the latest.
  -Dir <path>         Install into this directory.
  -Help               Show this help and exit.

Environment variables:
  OBORO_VERSION, OBORO_INSTALL_DIR, OBORO_FEATURES, OBORO_SKIP_CHECKSUM,
  OBORO_VERIFY_PROVENANCE. See the script header for details.
"@ | Write-Host
}

# oboro publishes one archive for Windows today: x86_64-pc-windows-msvc. An
# Arm64 machine is refused rather than handed a binary that will not run.
function Get-Target {
	$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
	switch ($architecture) {
		'X64' { return 'x86_64-pc-windows-msvc' }
		'Arm64' { Write-Fail "oboro does not yet ship an Arm64 Windows binary. Build from source with 'cargo install'." }
		default { Write-Fail "Unsupported architecture: $architecture." }
	}
}

# Follows the redirect from the HTML /releases/latest to /releases/tag/<tag>,
# for the same rate-limit reason get_latest_version in install.sh gives: this
# is not throttled the way api.github.com is for a shared IP address.
function Get-LatestVersion {
	$url = "https://github.com/$Repo/releases/latest"
	try {
		$response = Invoke-WebRequest -Uri $url -UseBasicParsing -Method Head -MaximumRedirection 5
	} catch {
		return $null
	}
	# Windows PowerShell 5.1's BaseResponse is a WebResponse, carrying
	# ResponseUri; PowerShell 7+'s is an HttpResponseMessage, carrying
	# RequestMessage.RequestUri instead. Neither type has both properties, and
	# Set-StrictMode turns a direct read of the absent one into an error, so
	# look each up through PSObject.Properties, which returns $null instead.
	$finalUrl = $null
	$baseResponse = $response.BaseResponse
	$responseUriProperty = $baseResponse.PSObject.Properties['ResponseUri']
	if ($responseUriProperty) {
		$finalUrl = $responseUriProperty.Value.ToString()
	} else {
		$requestMessageProperty = $baseResponse.PSObject.Properties['RequestMessage']
		if ($requestMessageProperty) {
			$finalUrl = $requestMessageProperty.Value.RequestUri.ToString()
		}
	}
	if ($finalUrl -match '/releases/tag/(.+)$') {
		return $Matches[1]
	}
	return $null
}

function Test-Checksum {
	param([string]$File, [string]$ChecksumsFile, [string]$FileName)

	if (-not (Test-Path $ChecksumsFile)) {
		Write-Fail 'SHA256SUMS is not available. Set OBORO_SKIP_CHECKSUM=1 to bypass.'
	}

	$expected = $null
	foreach ($line in Get-Content $ChecksumsFile) {
		$fields = $line -split '\s+', 2
		if ($fields.Count -lt 2) { continue }
		if ($fields[1].TrimStart('*') -eq $FileName) {
			$expected = $fields[0]
			break
		}
	}
	if (-not $expected) {
		Write-Fail "No checksum for $FileName in SHA256SUMS."
	}

	$actual = (Get-FileHash -Path $File -Algorithm SHA256).Hash
	if ($expected.ToUpperInvariant() -ne $actual.ToUpperInvariant()) {
		Write-Fail "Checksum verification failed.`n  Expected: $expected`n  Actual:   $actual"
	}
	Write-Info 'Checksum verified.'
}

function Test-Provenance {
	param([string]$File)
	if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
		Write-Fail 'OBORO_VERIFY_PROVENANCE=1 needs the gh CLI, which is not installed.'
	}
	Write-Info 'Verifying build provenance...'
	& gh attestation verify $File --repo $Repo
	if ($LASTEXITCODE -ne 0) {
		Write-Fail 'Build provenance verification failed.'
	}
}

function Main {
	[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
		Justification = 'Installer output belongs on the console with colour.')]
	param(
		[string]$Version,
		[string]$Dir,
		[string]$Features,
		[switch]$Help
	)

	if ($Help) {
		Show-Usage
		return
	}

	if ($Features) {
		Write-Fail "Unsupported feature build: $Features. No prebuilt Windows feature build is published; ner and ocr need 'cargo build --release --features <name>'."
	}

	Write-Info "Installing $BinaryName..."
	Write-Host ''

	$target = Get-Target

	$resolvedVersion = $Version
	if (-not $resolvedVersion) {
		Write-Info 'Resolving the latest release...'
		$resolvedVersion = Get-LatestVersion
		if (-not $resolvedVersion) {
			Write-Fail "Could not resolve the latest version. Pass -Version or see https://github.com/$Repo/releases."
		}
	}
	# The tags carry no leading v; accept one anyway so a pasted v0.7.0 works.
	$resolvedVersion = $resolvedVersion.TrimStart('v')

	$installDir = $Dir
	if (-not $installDir) {
		$installDir = Join-Path $env:LOCALAPPDATA "Programs\$BinaryName\bin"
	}

	Write-Info "Version:           $resolvedVersion"
	Write-Info "Target:            $target"
	Write-Info "Install directory: $installDir"
	Write-Host ''

	$filename = "$BinaryName-$resolvedVersion-$target.zip"
	$baseUrl = "https://github.com/$Repo/releases/download/$resolvedVersion"

	$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
	New-Item -ItemType Directory -Path $tmpDir | Out-Null

	try {
		Write-Info "Downloading $filename..."
		try {
			Invoke-WebRequest -Uri "$baseUrl/$filename" -OutFile (Join-Path $tmpDir $filename) -UseBasicParsing
		} catch {
			Write-Fail "Download failed. See https://github.com/$Repo/releases for available builds."
		}

		if ($env:OBORO_SKIP_CHECKSUM -eq '1') {
			Write-Warn 'Checksum verification skipped (OBORO_SKIP_CHECKSUM=1).'
		} else {
			try {
				Invoke-WebRequest -Uri "$baseUrl/SHA256SUMS" -OutFile (Join-Path $tmpDir 'SHA256SUMS') -UseBasicParsing
			} catch {
				Write-Fail 'Could not download SHA256SUMS. Set OBORO_SKIP_CHECKSUM=1 to bypass.'
			}
			Test-Checksum -File (Join-Path $tmpDir $filename) -ChecksumsFile (Join-Path $tmpDir 'SHA256SUMS') -FileName $filename
		}

		if ($env:OBORO_VERIFY_PROVENANCE -eq '1') {
			Test-Provenance -File (Join-Path $tmpDir $filename)
		}

		Write-Info 'Extracting...'
		Expand-Archive -Path (Join-Path $tmpDir $filename) -DestinationPath $tmpDir -Force
		$extracted = Join-Path $tmpDir "$BinaryName.exe"
		if (-not (Test-Path $extracted)) {
			Write-Fail "The archive did not contain a $BinaryName.exe binary."
		}

		New-Item -ItemType Directory -Path $installDir -Force | Out-Null
		$destination = Join-Path $installDir "$BinaryName.exe"
		try {
			Copy-Item -Path $extracted -Destination $destination -Force
		} catch {
			Write-Fail "Could not write to $installDir. Close any running $BinaryName process and try again."
		}

		Write-Host ''
		Write-Info "Installed $BinaryName $resolvedVersion to $destination."
		Write-Host ''

		$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
		$pathEntries = @()
		if ($userPath) { $pathEntries = $userPath -split ';' }
		if ($pathEntries -notcontains $installDir) {
			Write-Warn "$installDir is not on your PATH. Adding it..."
			$newPath = if ($userPath) { "$userPath;$installDir" } else { $installDir }
			[Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
			Write-Warn 'Open a new terminal for this to take effect.'
			Write-Host ''
		}

		Write-Host 'Next steps:'
		Write-Host "  $BinaryName doctor   # Report what this build can do"
		Write-Host "  $BinaryName --help   # List the commands"
		Write-Host "  $BinaryName completions powershell | Out-String | Invoke-Expression   # Complete commands in this session"
		Write-Host ''
		Write-Host 'This is the default feature set. Finding untold names (ner) and reading'
		Write-Host 'images (ocr) both need a source build; see https://m.canouil.dev/oboro/quickstart.html'
		Write-Host ''
		Write-Warn "Windows may warn that $BinaryName.exe is from an unidentified publisher on first run; the binary is unsigned but its release build is verified above."
	} finally {
		Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
	}
}

try {
	Main -Version $Version -Dir $Dir -Features $Features -Help:$Help
} catch {
	# Path is set only when this file is run directly (./install.ps1); piped
	# through iex or invoked as a scriptblock it is absent, and exiting the
	# process there would close the caller's session.
	#
	# Asked for through PSObject.Properties rather than read straight off the
	# object: under Set-StrictMode -Version Latest, reading a property that is
	# not there is itself a terminating error, so `$MyInvocation.MyCommand.Path`
	# would replace whatever went wrong above with a complaint about `Path`.
	$command = $MyInvocation.MyCommand
	if ($command.PSObject.Properties['Path'] -and $command.Path) {
		exit 1
	}
	throw
}
