param([int]$Pid0, [string]$Name)
# Presses a button in the app by its accessible name (UI Automation), so the
# mouse cursor is never moved.
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$p = Get-Process -Id $Pid0
$root = [System.Windows.Automation.AutomationElement]::FromHandle($p.MainWindowHandle)
$cond = New-Object System.Windows.Automation.AndCondition(
  (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, $Name)),
  (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)))
$el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
if (-not $el) {
  $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, $Name)
  $el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}
if (-not $el) { "not found: $Name"; exit 1 }
$pat = $null
if ($el.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pat)) { $pat.Invoke(); "invoked $Name" }
elseif ($el.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pat)) { $pat.Toggle(); "toggled $Name" }
elseif ($el.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pat)) { $pat.Select(); "selected $Name" }
else { "no pattern on $Name"; exit 1 }
