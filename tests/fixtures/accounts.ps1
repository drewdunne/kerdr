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
    switch ($request.step) {
      'start' { @{ok=$true;message='Authorize in your browser';status='waiting';ticket='fixture-ticket';url='https://example.com/signin';needs_code=$true} | ConvertTo-Json -Compress }
      'poll' { @{ok=$true;message='Waiting';status='waiting'} | ConvertTo-Json -Compress }
      'cancel' { @{ok=$true;message='cancelled';status='cancelled'} | ConvertTo-Json -Compress }
      default { @{ok=$true;message='Done';status='done'} | ConvertTo-Json -Compress }
    }
  }
  { $_ -in 'account-models','account-add','account-test','account-remove' } { @{ok=$true;message='Account action completed';status='ready'} | ConvertTo-Json -Compress }
  default { @{ok=$false;message='Unknown fixture action'} | ConvertTo-Json -Compress; exit 1 }
}
