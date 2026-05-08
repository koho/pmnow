# pmnow

Connect your PMSX003 sensor to a computer using a USB cable and run the command below to read the sensor value:

```shell
pmnow COM1 Home
```

The first argument is the serial port and the second is the display name of the sensor.

Command output:

```shell
ID: MTIzNDU2Nzg5MA==
PM: 17 (PM2.5) 19 (PM10)
```

You can view historical data at https://pmnow.netlify.app/sensor/{ID}.

## Screenshot

<img width="420" height="335" alt="ScreenShot_2026-05-08_171024_705" src="https://github.com/user-attachments/assets/95074461-ac59-4c6f-8ef9-0cc159b0de1f" />
