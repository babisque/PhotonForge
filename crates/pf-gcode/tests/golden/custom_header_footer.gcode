; PhotonForge job
G21 G90
M4 S0
M8 ; air assist on
; Layer Cut pass 1 of 1
G0 X10 Y10
G1 X20 Y10 F1000 S400
G1 X20 Y20
G1 X10 Y20
G1 X10 Y10
M9 ; air assist off
G0 X0 Y0
M5
