# Tuya (local)

- Aucun code repris : trames 3.3 / 3.4 / 3.5 vérifiées octet par octet contre la sortie
  d'implémentations existantes (localtuya, tinytuya), règle de tinytuya pour les en-têtes de version ; l'import lit le
  compte Tuya déjà lié à HA (`tools/home-assistant/tuya-export.py`).
- Pas de fixture d'appareil : les tests (`src/protocol.rs`) utilisent des trames produites
  par ces implémentations (vecteurs d'interopérabilité), pas des captures.
- Vérifié en vrai : import depuis HA, lecture et écriture locales sur des prises Tuya.
- Borne dé (`src/products.rs`) : lue en vrai (état, tension, consigne de courant changée depuis l'appli et vue) ;
  courant, puissance et énergie en charge pris des pilotes dé d'evcc et sowel, pas encore vus en charge ;
  marche/pause (DP 140) non essayée.
- Reste : la version 3.1 n'est pas gérée ; les sous-appareils derrière une passerelle sont
  ignorés ; l'import passe par HA ou par `tools/tuya/qr-export.py` (QR code).
