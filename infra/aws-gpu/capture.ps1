# Record the desktop and take screenshots. (Stills: ask-app.ps1 shows the hidden-window way to take
# one and upload it; use run-in-session.ps1, which hides the window, to start this script.) Must run INSIDE the logged-on desktop
# session (session 1), so start it through run-in-session.ps1, not directly from SSM.
#
#   capture.ps1 -Out C:\gpu\out -Seconds 60
param([string]$Out = "C:\gpu\out", [int]$Seconds = 60)
New-Item -ItemType Directory -Force $Out | Out-Null
$ff = "C:\gpu\ffmpeg.exe"
# gdigrab, not ddagrab (see bootstrap-box.ps1). 10 fps is enough for review.
Start-Process $ff -ArgumentList "-y","-hide_banner","-f","gdigrab","-framerate","10","-i","desktop",
    "-t","$Seconds","-c:v","libx264","-pix_fmt","yuv420p","$Out\screen.mp4" -RedirectStandardError "$Out\ffmpeg-video.log" -WindowStyle Hidden
