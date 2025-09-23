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
