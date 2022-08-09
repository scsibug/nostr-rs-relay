#!/bin/bash

osascript <<EOS

on toggle()
	tell application "System Preferences" to reveal pane id "com.apple.preferences.sharing"

	tell application "System Events" to tell window 1 of application process "System Preferences"

		repeat until exists checkbox 1 of (first row of table 1 of scroll area 1 of group 1 whose value of static text 1 is "AirPlay Receiver")
			delay 0.1
		end repeat
		if value of checkbox 1 of (first row of table 1 of scroll area 1 of group 1 whose value of static text 1 is "AirPlay Receiver") as boolean then
			click checkbox 1 of (first row of table 1 of scroll area 1 of group 1 whose value of static text 1 is "AirPlay Receiver")
		else
			click checkbox 1 of (first row of table 1 of scroll area 1 of group 1 whose value of static text 1 is "AirPlay Receiver")
		end if
	end tell
end toggle

if application "System Preferences" is not running then
	tell application "System Preferences" to activate
	toggle()
	tell application "System Preferences" to quit
else
	toggle()
	tell application "System Preferences" to quit
end if

EOS
