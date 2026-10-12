// FindBinary(Name) searches these locations in order:
//   1. $(CARGO_HOME)\bin
//   2. $(USERPROFILE)\.cargo\bin (fallback when CARGO_HOME not set)
//   3. $(CARGO_TARGET_DIR)\release
//   4. <script>\..\target\release  (project root target)
//   5. <script>\rusbmux-usb-win-driver\target\release

#define BIN_LOC1 GetEnv("CARGO_HOME") + "\bin\"
#define BIN_LOC2 GetEnv("USERPROFILE") + "\.cargo\bin\"
#define BIN_LOC3 GetEnv("CARGO_TARGET_DIR") + "\release\"
#define BIN_LOC4 SourcePath + "..\target\release\"
#define BIN_LOC5 SourcePath + "rusbmux-usb-win-driver\target\release\"

#define FindBinary(Name) \
  FileExists(BIN_LOC1 + Name) ? BIN_LOC1 + Name : \
  FileExists(BIN_LOC2 + Name) ? BIN_LOC2 + Name : \
  FileExists(BIN_LOC3 + Name) ? BIN_LOC3 + Name : \
  FileExists(BIN_LOC4 + Name) ? BIN_LOC4 + Name : \
  BIN_LOC5 + Name


#ifnexist FindBinary("shawl.exe")
  #error shawl.exe not found. Install with: cargo install shawl
#endif

#ifnexist FindBinary("rusbmux.exe")
  #error rusbmux.exe not found. Build with: cargo build --release -F rusb,rusb-vendored,bin --no-default-features
#endif

#ifnexist FindBinary("rusbmux-usb-win-driver.exe")
  #error rusbmux-usb-win-driver.exe not found. Build with: cargo build --release
#endif

// application version extraction from Cargo.toml
// reads up to 20 lines and extracts `version = "X.Y.Z"`

#define CargoFile SourcePath + "..\Cargo.toml"
#define CargoHandle FileOpen(CargoFile)

#define _C1 FileRead(CargoHandle)
#define _C2 FileRead(CargoHandle)
#define _C3 FileRead(CargoHandle)
#define _C4 FileRead(CargoHandle)
#define _C5 FileRead(CargoHandle)
#define _C6 FileRead(CargoHandle)
#define _C7 FileRead(CargoHandle)
#define _C8 FileRead(CargoHandle)
#define _C9 FileRead(CargoHandle)
#define _C10 FileRead(CargoHandle)
#define _C11 FileRead(CargoHandle)
#define _C12 FileRead(CargoHandle)
#define _C13 FileRead(CargoHandle)
#define _C14 FileRead(CargoHandle)
#define _C15 FileRead(CargoHandle)
#define _C16 FileRead(CargoHandle)
#define _C17 FileRead(CargoHandle)
#define _C18 FileRead(CargoHandle)
#define _C19 FileRead(CargoHandle)
#define _C20 FileRead(CargoHandle)

#define _CargoFileClosed FileClose(CargoHandle)

// extract version from `version = "X.Y.Z"`
#define _VersionFromLine(Ln) \
  Pos('version = "', Ln) > 0 ? \
    Copy( \
      Copy(Ln, Pos('"', Ln) + 1, Len(Ln)), \
      1, \
      Pos('"', Copy(Ln, Pos('"', Ln) + 1, Len(Ln))) - 1 \
    ) \
    : ""

// walk lines, keep the first version found
#define _V1 _VersionFromLine(_C1)
#define _V2 Len(_V1) > 0 ? _V1 : _VersionFromLine(_C2)
#define _V3 Len(_V2) > 0 ? _V2 : _VersionFromLine(_C3)
#define _V4 Len(_V3) > 0 ? _V3 : _VersionFromLine(_C4)
#define _V5 Len(_V4) > 0 ? _V4 : _VersionFromLine(_C5)
#define _V6 Len(_V5) > 0 ? _V5 : _VersionFromLine(_C6)
#define _V7 Len(_V6) > 0 ? _V6 : _VersionFromLine(_C7)
#define _V8 Len(_V7) > 0 ? _V7 : _VersionFromLine(_C8)
#define _V9 Len(_V8) > 0 ? _V8 : _VersionFromLine(_C9)
#define _V10 Len(_V9) > 0 ? _V9 : _VersionFromLine(_C10)
#define _V11 Len(_V10) > 0 ? _V10 : _VersionFromLine(_C11)
#define _V12 Len(_V11) > 0 ? _V11 : _VersionFromLine(_C12)
#define _V13 Len(_V12) > 0 ? _V12 : _VersionFromLine(_C13)
#define _V14 Len(_V13) > 0 ? _V13 : _VersionFromLine(_C14)
#define _V15 Len(_V14) > 0 ? _V14 : _VersionFromLine(_C15)
#define _V16 Len(_V15) > 0 ? _V15 : _VersionFromLine(_C16)
#define _V17 Len(_V16) > 0 ? _V16 : _VersionFromLine(_C17)
#define _V18 Len(_V17) > 0 ? _V17 : _VersionFromLine(_C18)
#define _V19 Len(_V18) > 0 ? _V18 : _VersionFromLine(_C19)
#define _V20 Len(_V19) > 0 ? _V19 : _VersionFromLine(_C20)

#define AppVersion _V20

#if AppVersion == ""
  #error Could not parse version from Cargo.toml. Look for: version = "X.Y.Z"
#endif

#define AppName 'rusbmux'
#define AppleServiceName 'Apple Mobile Device Service'

// error codes from rusbmux-usb-win-driver.exe
#define EXIT_SUCCESS              0
#define ERR_DRIVER_GENERIC       -67
#define ERR_DRIVER_FETCH_DEVICES -667
#define ERR_DRIVER_NO_DEVICE     -677
#define ERR_REMOVE_ENUM_DEVICES  67
#define ERR_REMOVE_SCAN_DEVICES  667
#define ERR_REMOVE_DELETE_DRIVER 677

[Setup]
AppName={#AppName}
AppVersion={#AppVersion}
AppComments=A usbmuxd replacement in pure Rust.
AppPublisher=Abdullah Al-Banna
AppPublisherURL=https://github.com/abdubanna/rusbmux
AppReadmeFile=https://github.com/abdubanna/rusbmux/blob/main/README.md

// a placeholder so Inno Setup creates the page, the content is replaced at runtime
LicenseFile=..\LICENSE-MIT

OutputBaseFilename={#AppName}-{#AppVersion}-x86_64-setup
WizardStyle=dynamic
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
OutputDir=dist
DisableWelcomePage=no
DisableDirPage=no
CloseApplications=yes
AppMutex={#AppName}
UninstallDisplayName={#AppName}
PrivilegesRequired=admin

[Types]
Name: "full"; Description: "Full installation"
Name: "service"; Description: "Service only (skip driver)"
Name: "driver"; Description: "Driver only (skip service)"

[Components]
Name: "full"; Description: "Full"; Types: full
Name: "service"; Description: "Service"; Types: full service
Name: "driver"; Description: "Driver"; Types: full driver

[Tasks]
Name: "cleandrivers"; Description: "Remove existing drivers for Apple devices before installation"; Components: driver; Flags: checkablealone
Name: "debugusbinstall"; Description: "Debug the USB driver installation"; Components: driver; Flags: unchecked checkedonce
Name: "deleteservice"; Description: "Remove existing Apple service"; Components: service; Flags: checkablealone
Name: "imitateappleservice"; Description: "Imitate the original Apple service (helps with apps that require the same service name, e.g. 3uTools, iTunes, ..etc)"; Components: service; Flags: checkablealone

[Files]
Source: "..\README.md"; DestDir: "{app}"; Flags: isreadme
Source: "..\LICENSE-MIT"; DestDir: "{app}";
Source: "..\LICENSE-APACHE"; DestDir: "{app}";
Source: "..\THIRD_PARTY_LICENSES"; DestDir: "{app}";

// embedded license files for runtime display (combined in license page UI)
Source: "..\LICENSE-MIT"; DestDir: "{tmp}"; Flags: dontcopy
Source: "..\LICENSE-APACHE"; DestDir: "{tmp}"; Flags: dontcopy

Source: "{#FindBinary("rusbmux.exe")}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#FindBinary("rusbmux-usb-win-driver.exe")}"; DestDir: "{app}"; Components: driver; Flags: ignoreversion
Source: "{#FindBinary("shawl.exe")}"; DestDir: "{app}"; Components: service; Flags: ignoreversion

// remove the whole folder when uninstalling
[UninstallDelete]
Type: filesandordirs; Name: "{app}"

[Messages]
WelcomeLabel1=Welcome to {#AppName} v{#AppVersion}
WelcomeLabel2=This installer will guide you through installing {#AppName} on your system.

SelectDirDesc=Choose where {#AppName} will be installed.
SelectComponentsDesc=Select what parts of {#AppName} you want to install.

LicenseAccepted=I accept at least one of the available licenses
LicenseNotAccepted=I do not accept any of the license terms

FinishedLabel={#AppName} installation complete

[Code]

function ScExec(const Args: String): Boolean;
var
  ResultCode: Integer;
begin
  Result := Exec(
    ExpandConstant('{sys}\sc.exe'),
    Args,
    '',
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode
  ) and (ResultCode = 0);
end;

function ServiceExists(const ServiceName: string): Boolean;
begin
  Result := ScExec(Format('query "%s"', [ServiceName]));
end;

function GetServiceName(): String;
begin
  if IsTaskSelected('imitateappleservice') then
    Result := '{#AppleServiceName}'
  else
    Result := '{#AppName}';
end;

procedure DeleteServiceIfExists(const ServiceName: string);
begin
  if ServiceExists(ServiceName) then
  begin
    ScExec(Format('stop "%s"', [ServiceName]));
    ScExec(Format('delete "%s"', [ServiceName]));
  end;
end;

procedure BuildLicenseText;
var
  MITText, ApacheText: AnsiString;
begin
  ExtractTemporaryFile('LICENSE-MIT');
  ExtractTemporaryFile('LICENSE-APACHE');

  LoadStringFromFile(ExpandConstant('{tmp}\LICENSE-MIT'), MITText);
  LoadStringFromFile(ExpandConstant('{tmp}\LICENSE-APACHE'), ApacheText);

  WizardForm.LicenseMemo.Lines.Text :=
    'This software is available under two licenses.' + #13#10#13#10 +

    'You may choose either license when using or redistributing this software.' + #13#10#13#10 +

    '--- MIT LICENSE ---' + #13#10#13#10 +
    MITText + #13#10#13#10 +

    '--- APACHE LICENSE 2.0 ---' + #13#10#13#10 +
    ApacheText + #13#10#13#10 +

    'By continuing installation, you acknowledge acceptance of at least one of these licenses.';
end;

// check if a service belongs to us by inspecting its ImagePath in the registry.
function IsRusbmuxService(const ServiceName: String): Boolean;
var
  ImagePath: String;
begin
  Result := False;

  if not RegQueryStringValue(
    HKLM,
    Format('SYSTEM\CurrentControlSet\Services\%s', [ServiceName]),
    'ImagePath',
    ImagePath
  ) then
    Exit;

  Result := Pos(LowerCase(ExpandConstant('{app}')), LowerCase(ImagePath)) > 0;
end;

function CreateService(const ServiceName: String): Boolean;
var
  ResultCode: Integer;
begin
  Result := False;

  if not Exec(
    ExpandConstant('{app}\shawl.exe'),
    Format('add --name "%s" --dependencies Tcpip -- "%s"', [
      ServiceName,
      ExpandConstant('{app}\rusbmux.exe')
    ]),
    '',
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode
  ) or (ResultCode <> 0) then
  begin
    MsgBox(
      Format('Failed to create the "%s" service with shawl.', [ServiceName]),
      mbError,
      MB_OK
    );
    Exit;
  end;

  Result := True;
end;

function ConfigureServiceAutoStart(const ServiceName: String): Boolean;
begin
  Result := ScExec(Format('config "%s" start= auto', [ServiceName]));
  if not Result then
    MsgBox(
      Format('Failed to set the "%s" service to auto-start.', [ServiceName]),
      mbError,
      MB_OK
    );
end;

function ConfigureServiceUser(const ServiceName: String): Boolean;
begin
  Result := ScExec(Format('config "%s" obj= "NT AUTHORITY\LocalService"', [ServiceName]));
  if not Result then
    MsgBox(
      Format('Failed to set the user for the "%s" service.', [ServiceName]),
      mbError,
      MB_OK
    );
end;

// reset failure counter everyday and restart on failure after 5 seconds
function ConfigureServiceFailure(const ServiceName: String): Boolean;
begin
  Result := ScExec(Format(
    'failure "%s" reset= 86400 actions= restart/50000/restart/50000/restart/50000', [ServiceName]
  ));
  if not Result then
    MsgBox(
      Format('Failed to set failure recovery for the "%s" service.', [ServiceName]),
      mbError,
      MB_OK
    );
end;

function SetServiceDescription(const ServiceName: String): Boolean;
begin
  Result := ScExec(Format(
    'description "%s" "Provides a usbmuxd-compatible interface for communication with Apple mobile devices over USB or Network"', [ServiceName]
  ));
  if not Result then
    MsgBox(
      Format('Failed to set the description for the "%s" service.', [ServiceName]),
      mbError,
      MB_OK
    );
end;

function StartService(const ServiceName: String): Boolean;
begin
  Result := ScExec(Format('start "%s"', [ServiceName]));
  if not Result then
    MsgBox(
      Format('Failed to start the "%s" service.', [ServiceName]),
      mbError,
      MB_OK
    );
end;

function InstallService(): Boolean;
var
  ServiceName: String;
begin
  Result := False;
  ServiceName := GetServiceName();

  DeleteServiceIfExists(ServiceName);

  if not CreateService(ServiceName) then
    Exit;

  if not ConfigureServiceAutoStart(ServiceName) then
    Exit;

  if not ConfigureServiceUser(ServiceName) then
    Exit;

  if not ConfigureServiceFailure(ServiceName) then
    Exit;

  if not SetServiceDescription(ServiceName) then
    Exit;

  if not StartService(ServiceName) then
    Exit;

  Result := True;
end;

function RunDriverInstaller(const Params: string): Boolean;
var
  ShowCode: Integer;
  ResultCode: Integer;
  FullParams: String;
begin
  if (not IsUninstaller()) and IsTaskSelected('debugusbinstall') then
  begin
    ShowCode := SW_SHOW;
    FullParams := Params + ' --wait';
  end
  else
  begin
    ShowCode := SW_HIDE;
    FullParams := Params;
  end;

  Result := Exec(
    ExpandConstant('{app}\rusbmux-usb-win-driver.exe'),
    FullParams,
    '',
    ShowCode,
    ewWaitUntilTerminated,
    ResultCode
  );

  if not Result then
  begin
    MsgBox('Failed to start driver installer.', mbError, MB_OK);
    Exit;
  end;

  case ResultCode of
    {#EXIT_SUCCESS}:
      Result := True;

    {#ERR_DRIVER_FETCH_DEVICES}:
      begin
        MsgBox(Format('Driver install failed: Could not fetch connected devices (error code: %d).', [ResultCode]), mbError, MB_OK);
        Result := False;
      end;

    {#ERR_DRIVER_NO_DEVICE}:
      begin
        MsgBox(
          'Driver install failed: No Apple device is plugged in, please plug one and rerun the setup' + #13#10 + #13#10 +
          'This is a one time install, you do not need to rerun this for each device',
          mbError,
          MB_OK
        );
        Result := False;
      end;

    {#ERR_DRIVER_GENERIC}:
      begin
        MsgBox(Format('Driver install failed with error code: %d.', [ResultCode]), mbError, MB_OK);
        Result := False;
      end;

    {#ERR_REMOVE_ENUM_DEVICES}:
      begin
        MsgBox('Driver removal failed: Could not enumerate devices', mbError, MB_OK);
        Result := False;
      end;

    {#ERR_REMOVE_SCAN_DEVICES}:
      begin
        MsgBox('Driver removal failed: Could not rescan devices', mbError, MB_OK);
        Result := False;
      end;

    {#ERR_REMOVE_DELETE_DRIVER}:
      begin
        MsgBox('Driver removal failed: Could not delete driver', mbError, MB_OK);
        Result := False;
      end;

  else
    begin
      MsgBox(
        Format('Driver install failed with unknown error code: %d.', [ResultCode]),
        mbError,
        MB_OK
      );
      Result := False;
    end;
  end;
end;

function InstallUsbDriver(): Boolean;
var
  Params: String;
begin
  Params := '--install';

  if IsTaskSelected('cleandrivers') then
    Params := Params + ' --clean';

  Result := RunDriverInstaller(Params);
end;

procedure InitializeWizard;
begin
  BuildLicenseText;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin

  if CurStep = ssInstall then
  begin
    WizardForm.StatusLabel.Caption := 'Deleting previous service if exists...';

    if IsRusbmuxService('{#AppName}') then
      DeleteServiceIfExists('{#AppName}');

    if IsRusbmuxService('{#AppleServiceName}') then
      DeleteServiceIfExists('{#AppleServiceName}');
  end;

  if CurStep = ssPostInstall then
  begin

    if IsComponentSelected('driver') then
    begin
      WizardForm.StatusLabel.Caption := 'Installing the USB driver...';

      if not InstallUsbDriver() then
        Exit;
    end;

    if IsComponentSelected('service') then
    begin
      if IsTaskSelected('deleteservice') then
      begin
        WizardForm.StatusLabel.Caption := 'Removing the Apple service...';
        DeleteServiceIfExists('{#AppleServiceName}');
      end;

      WizardForm.StatusLabel.Caption := 'Installing the rusbmux service...';

      if not InstallService() then
        Exit;
    end;

  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    if ServiceExists('{#AppleServiceName}') or ServiceExists('{#AppName}') then
    begin
      UninstallProgressForm.StatusLabel.Caption := 'Removing the rusbmux service...';

      if IsRusbmuxService('{#AppName}') then
        DeleteServiceIfExists('{#AppName}');

      if IsRusbmuxService('{#AppleServiceName}') then
        DeleteServiceIfExists('{#AppleServiceName}');

      UninstallProgressForm.ProgressBar.Position := UninstallProgressForm.ProgressBar.Max / 2;
    end;

    if FileExists(ExpandConstant('{app}\rusbmux-usb-win-driver.exe')) then
    begin
      UninstallProgressForm.StatusLabel.Caption := 'Removing the rusbmux USB driver...';
      RunDriverInstaller('--clean --rescans 1');
      UninstallProgressForm.ProgressBar.Position := UninstallProgressForm.ProgressBar.Max;
    end;

  end;
end;
