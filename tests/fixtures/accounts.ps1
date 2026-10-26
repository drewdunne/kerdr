$ErrorActionPreference = 'Stop'
$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
switch ($request.action) {
  'account-move' {
    if ($request.conversation -eq 'stale-conversation') { @{ok=$true;state='stale';message='the conversation changed'} | ConvertTo-Json -Compress }
    elseif ($request.confirm -eq 'fixture-confirm') { @{ok=$true;state='queued';account='ARK';message='moving to ARK'} | ConvertTo-Json -Compress }
    else { @{ok=$false;message='move?';confirmation='Mid-step; move when idle and end background jobs?';yes='move when it stops';no='leave it';confirm='fixture-confirm'} | ConvertTo-Json -Compress }
  }
  'account-move-waiting' { @{ok=$true;message='Waiting sessions moved';moved=2} | ConvertTo-Json -Compress }
  'account-signin' {
    # A copy other than the agents' one gets its own ticket, and refuses a pasted token.
    if ($request.copy -eq 'tenants' -and $request.step -eq 'start') { @{ok=$true;message='Authorize the tenants copy';status='waiting';ticket='fixture-tenants-ticket';url='https://example.com/signin';needs_code=$true} | ConvertTo-Json -Compress; break }
    if ($request.copy -eq 'tenants' -and $request.step -eq 'paste') { @{ok=$false;message='The tenants copy signs in on its machine, not by paste';status='failed'} | ConvertTo-Json -Compress; break }
    switch ($request.step) {
      'start' { @{ok=$true;message='Authorize in your browser';status='waiting';ticket='fixture-ticket';url='https://example.com/signin';needs_code=$true} | ConvertTo-Json -Compress }
      'poll' { @{ok=$true;message='Waiting';status='waiting'} | ConvertTo-Json -Compress }
      'cancel' { @{ok=$true;message='cancelled';status='cancelled'} | ConvertTo-Json -Compress }
      default { @{ok=$true;message='Done';status='done'} | ConvertTo-Json -Compress }
    }
  }
  { $_ -eq 'account-test' -and $request.copy -eq 'tenants' } { @{ok=$true;message='The tenants copy works';status='ready'} | ConvertTo-Json -Compress; break }
  { $_ -in 'account-models','account-add','account-test','account-remove' } { @{ok=$true;message='Account action completed';status='ready'} | ConvertTo-Json -Compress }
  default { @{ok=$false;message='Unknown fixture action'} | ConvertTo-Json -Compress; exit 1 }
}
