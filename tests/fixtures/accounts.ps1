$ErrorActionPreference = 'Stop'
$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
switch ($request.action) {
  'account-move' { @{ok=$false;message='move?';confirmation='Mid-step; move when idle and end background jobs?';yes='move when it stops';no='leave it';confirm='fixture-confirm'} | ConvertTo-Json -Compress }
  'account-move-waiting' { @{ok=$true;message='Waiting sessions moved';moved=2} | ConvertTo-Json -Compress }
  'account-signin' {
    switch ($request.step) {
      'start' { @{ok=$true;message='Authorize in your browser';status='waiting';ticket='fixture-ticket';url='https://example.com/signin';needs_code=$true} | ConvertTo-Json -Compress }
      'poll' { @{ok=$true;message='Waiting';status='waiting'} | ConvertTo-Json -Compress }
      default { @{ok=$true;message='Done';status='done'} | ConvertTo-Json -Compress }
    }
  }
  { $_ -in 'account-models','account-add','account-test','account-remove' } { @{ok=$true;message='Account action completed';status='ready'} | ConvertTo-Json -Compress }
  default { @{ok=$false;message='Unknown fixture action'} | ConvertTo-Json -Compress; exit 1 }
}
