# Z-Wave (via Z-Wave JS UI)

Les modules Z-Wave (relais, prises, capteurs, serrures…) par une clé Z-Wave USB et
Z-Wave JS UI. Z-Wave JS UI pilote la clé, inclut et règle les modules (son interface web,
liée depuis Moli : « Appairage et réglages ») ; Moli lit son serveur Z-Wave JS (websocket
local, port 3000) : tout le réseau au démarrage, puis chaque changement.

- **Identité** : `<instance>:<home id>-<nœud>` (une autre clé = un autre réseau).
- **Points** : chaque valeur lisible, par son sens (température, puissance, batterie…) ;
  `currentValue` et `targetValue` d'un interrupteur ou d'un variateur font un seul point
  (`switch`, `level`), lu sur l'un, écrit sur l'autre. Les nombres nommés deviennent des
  choix. Réglages (paramètres), versions, associations restent dans Z-Wave JS UI.
- **Hors ligne** : un nœud « mort » ; un nœud sur pile endormi reste en ligne.
- **Limites** : la fixture est écrite d'après le protocole documenté, pas encore capturée
  sur un vrai réseau ; les couleurs et les durées ne sont pas des points.
