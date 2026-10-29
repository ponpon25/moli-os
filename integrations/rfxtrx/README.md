# Capteurs 433 MHz (RFXtrx)

Les sondes de température et d'humidité sur 433 MHz (Oregon Scientific, La Crosse, TFA,
Viking, Rubicson…) par un émetteur-récepteur RFXCOM RFXtrx433 branché en USB. Local, en
lecture seule. Moli remet la clé à zéro, lit son état (fréquence, firmware, protocoles
écoutés), lance la réception, puis lit chaque trame.

- **Identité** : le code de la sonde sur l'air, `<type>-<sous-type>-<id>` (`52-01-1a2b`),
  visible dans la description de l'appareil. Une sonde Oregon **change de code à chaque
  changement de piles** : la déclarer sous un id stable (`[[driver.options.sensor]]`) et
  ne changer que son `code` ensuite ; l'appareil et son historique restent.
- **Points** : `temperature` (°C), `humidity` (%), `pressure` (hPa) selon la sonde,
  `battery_low`, `rssi` (0 à 15).
- **Hors ligne** après `offline_after` minutes sans trame (30 par défaut).
- **Limites** : les protocoles écoutés sont ceux gardés en mémoire par la clé (RFXmngr
  les change) ; seules les sondes d'air sont lues pour l'instant. Une trame d'un autre
  type est nommée une fois dans les journaux (`heard packet type …, not read by Moli`).
