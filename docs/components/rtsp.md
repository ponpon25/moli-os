# Les caméras en RTSP

Moli peut **resservir en RTSP** les caméras qu'il sait lire, pour qu'un enregistreur (UniFi Protect, un NVR)
ou un lecteur (VLC, ffmpeg) les montre et les garde. Le cas d'origine : une sonnette HomeKit, que seul Moli
sait lire (il en est le contrôleur), et que l'on veut voir à côté des autres caméras dans Protect.

C'est une **surface**, comme HomeKit : elle ne commande rien, elle montre des caméras choisies une par une.
Code : `crates/moli-rtsp`.

## Configurer

Dans `moli.toml` :

```toml
[rtsp]
port = 8554        # défaut
user = "moli"      # défaut

[[rtsp.camera]]
path = "sonnette"                 # rtsp://<hôte>:8554/sonnette
device = "<instance>:<id natif>"  # un appareil qui a la vidéo en direct
```

Le mot de passe va dans le coffre, jamais dans le fichier :

```
printf '%s' "<mot de passe>" | moli-os secrets set rtsp password
```

Il est relu à chaque requête : le définir ou le changer compte aussitôt, sans redémarrer. Tant qu'il n'y en a
pas, personne n'entre. Seules les caméras listées sont servies.

Essayer avec VLC : `rtsp://moli:<mot de passe>@<hôte>:8554/sonnette` (ou ffmpeg :
`ffplay -rtsp_transport tcp rtsp://…`).

## Ce qui sort

- **La vidéo H.264 de la caméra, telle quelle** : ni décodée ni recodée, seulement remise en paquets RTP
  (STAP-A pour SPS/PPS, FU-A au-delà de 1 400 octets, mode 1, découpage commun avec HomeKit dans
  `moli-net/src/h264.rs`). Le SDP donne SPS et PPS (`sprop-parameter-sets`) dès qu'ils sont connus.
- **Sur la connexion RTSP elle-même** (RTP entrelacé, `RTP/AVP/TCP`). Un client qui ne propose que l'UDP
  reçoit `461 Unsupported Transport` (VLC et ffmpeg réessaient alors en TCP).
- **Pas de son** pour l'instant.

## En continu

Un enregistreur regarde pendant des jours, un flux de caméra ne dure pas autant (la sonnette HomeKit lâche
un spectateur au bout de dix minutes, une caméra décroche parfois du Wi-Fi). Pour chaque client :

- toutes les **8 minutes**, un second flux est ouvert à côté du premier, et pris à sa **première image
  clé** : pas de trou. Sur une source qui partage son flux (la sonnette HomeKit), ce second flux rejoint
  celui en cours : la caméra ne voit qu'une connexion ;
- un flux qui s'arrête quand même est **rouvert** (1 s, puis 2, 4… jusqu'à 15 s d'attente) ;
- les horodatages forment **une seule ligne de temps** pour le client, qui ne recule jamais ;
- sans aucune image pendant **une minute**, la connexion est fermée : l'enregistreur rappellera.

Chaque flux est lu par sa propre tâche dans une petite file : un client lent ne ralentit pas la caméra (les
images en trop sont sautées jusqu'à la prochaine image clé), une caméra lente ne bloque pas le client.

## La garde

Comme pour HomeKit, **une caméra d'une pièce protégée n'est jamais servie** : `DESCRIBE` répond `403`, et un
flux en cours est coupé dans les 5 s si la pièce devient protégée. Pas de demande d'approbation (une
approbation valide un ordre, pas un regard).

## Sécurité

- Mot de passe **Digest** (MD5, le mot de passe ne circule pas), nonce neuf par connexion ; seule `OPTIONS`
  répond sans. Trois mauvais mots de passe et la connexion est fermée, une seconde d'attente après chacun.
  Aucune caméra n'est ouverte avant le bon mot de passe.
- Bornes : 8 clients à la fois, en-têtes de 8 Kio, corps de 4 Kio, 60 s de silence avant `PLAY`, 10 s pour
  qu'un client prenne une image.
- Le RTSP n'est pas chiffré : la vidéo circule en clair sur le réseau local, comme celle de la plupart des
  caméras. À ne pas ouvrir sur Internet.

## UniFi Protect

Protect n'adopte que des caméras **ONVIF**. Cette surface fournit le flux RTSP ; la couche ONVIF
(découverte, profils, adresse du flux) n'est pas encore là. En attendant, un serveur ONVIF virtuel peut
envelopper l'adresse RTSP. Ce que Protect en fera : direct et enregistrement, sans sonnerie ni réponse au
visiteur (ce n'est pour lui qu'une caméra).

## Limites et essais

- Essayé : un client RTSP de bout en bout contre la surface et une fausse caméra (`src/tests.rs` : mot de
  passe, SDP, transport, flux qui démarre sur une image clé au numéro de séquence et à l'horodatage
  annoncés, sans perte, et qui continue quand le flux de la caméra s'arrête et est rouvert ; mauvais mots de
  passe, chemin inconnu, pièce protégée).
- Pas encore essayé : VLC et la vraie sonnette sur plusieurs heures, puis Protect.
