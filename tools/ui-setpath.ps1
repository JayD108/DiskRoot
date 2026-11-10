param([int]$Pid0, [string]$Value)
# Types into the app's path box via UI Automation (no keyboard/mouse use).
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$p = Get-Process -Id $Pid0
$root = [System.Windows.Automation.AutomationElement]::FromHandle($p.MainWindowHandle)
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit)
$el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
if (-not $el) { $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::ComboBox); $el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond) }
if (-not $el) { "no edit box"; exit 1 }
$pat = $null
if ($el.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pat)) { $pat.SetValue($Value); "set $Value" } else { "no value pattern"; exit 1 }
