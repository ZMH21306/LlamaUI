Set ws = CreateObject("WScript.Shell")
Dim scriptPath
scriptPath = Replace(WScript.ScriptFullName, WScript.ScriptName, "")
ws.Run Chr(34) & scriptPath & "run_hidden.ps1" & Chr(34), 0, False
