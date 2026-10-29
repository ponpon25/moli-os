# Onboarding : capteurs 433 MHz (RFXtrx)

1. Brancher la clé RFXtrx433 ; la passer au conteneur de Moli par son nom stable
   (`devices: - /dev/serial/by-id/usb-RFXCOM_RFXtrx433_…-if00-port0:/dev/rfxtrx`) avec le
   groupe du port série (`group_add`, `dialout`).
2. Déclarer `kind = "rfxtrx"` (aucune option requise), `moli-os check-config`, redémarrer.
   Les journaux donnent le firmware et les protocoles écoutés.
3. Chaque sonde entendue apparaît dans Équipements en une à deux minutes, sous son code.
   Lui donner un nom et une pièce. Une sonde d'un voisin : la mettre dans `ignore`.
4. Pour une sonde Oregon (nouveau code à chaque changement de piles) : la déclarer avec
   `[[driver.options.sensor]]` (`id`, `code`, `name`) ; après un changement de piles,
   remplacer seulement `code` par le nouveau (journaux : `new sensor …`).
