## Fixes

- **GPU stays off after a graphics-driver reset:** when the window stopped drawing after the GPU was lost for the first time in a session, the next start didn't switch to rendering on the CPU and the message didn't say the GPU was lost; only a loss after the recoveries had run out was recorded.
