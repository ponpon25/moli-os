# Les caméras en RTSP et ONVIF

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
name = "Sonnette"                 # son nom pour l'enregistreur (défaut : path)
onvif_port = 8899                 # aussi une caméra ONVIF, sur ce port HTTP (pour Protect)
```

`discovery = false` dans `[rtsp]` coupe la découverte WS-Discovery (port UDP 3702, active par défaut dès
qu'une caméra a un `onvif_port`).

Le mot de passe va dans le coffre, jamais dans le fichier :

```
printf '%s' "<mot de passe>" | moli-os secrets set rtsp password
```

puis redémarrer Moli (la commande écrit le coffre à côté du processus qui tourne, qui le garde en mémoire).
Tant qu'il n'y a pas de mot de passe, personne n'entre (la surface démarre quand même, et le dit dans le
journal). Seules les caméras listées sont servies.

Essayer avec VLC : `rtsp://moli:<mot de passe>@<hôte>:8554/sonnette` (ou ffmpeg :
`ffplay -rtsp_transport tcp rtsp://…`).

## Ce qui sort

- **La vidéo H.264 de la caméra, telle quelle** : ni décodée ni recodée, seulement remise en paquets RTP
  (STAP-A pour SPS/PPS, FU-A au-delà de 1 400 octets, mode 1, découpage commun avec HomeKit dans
  `moli-net/src/h264.rs`). Le SDP donne SPS et PPS (`sprop-parameter-sets`) dès qu'ils sont connus.
- **Comme le client le demande** : entrelacée sur la connexion RTSP (`RTP/AVP/TCP`), ou en UDP vers ses
  ports (`client_port`, unicast seulement, vers l'adresse de la connexion RTSP, jamais une autre, ports ≥
  1024). Un client UDP doit se manifester (une requête, ou ses rapports RTCP) au moins toutes les 2 minutes.
- **Un rapport d'émetteur RTCP** toutes les 5 s (horloge de la caméra contre l'heure réelle).
- **Pas de son** pour l'instant.

## En continu

Un enregistreur regarde pendant des jours, un flux de caméra ne dure pas autant (la sonnette HomeKit lâche
un spectateur au bout de dix minutes, une caméra décroche parfois du Wi-Fi). Pour chaque client :

- toutes les **8 minutes**, un second flux est ouvert à côté du premier, et pris à sa **première image
  clé** : pas de trou. Sur une source qui partage son flux (la sonnette HomeKit), ce second flux rejoint
  celui en cours : la caméra ne voit qu'une connexion ;
- un flux qui s'arrête quand même est **rouvert** (1 s, puis 2, 4… jusqu'à 15 s d'attente) ;
- les horodatages forment **une seule ligne de temps** pour le client, qui ne recule jamais. Deux flux
  d'une même source partagée portent les mêmes horodatages : la ligne continue telle quelle, et les images
  déjà envoyées par l'ancien flux ne sont pas renvoyées (sans cela, l'image clé de la bascule partait deux
  fois : vérifié par le test des 21 minutes) ;
- sans aucune image pendant **une minute**, la connexion est fermée : l'enregistreur rappellera.

Toutes les 5 minutes, chaque client laisse une ligne `rtsp: feed` au journal : images par seconde, images
clés, plus grand trou entre deux images, renouvellements, réouvertures, images sautées. Une réouverture
laisse aussi `rtsp: a new stream starts` avec la durée du trou. C'est ainsi qu'on suit un enregistreur sur
plusieurs jours.

Chaque flux est lu par sa propre tâche dans une petite file : un client lent ne ralentit pas la caméra (les
images en trop sont sautées jusqu'à la prochaine image clé), une caméra lente ne bloque pas le client.

## La garde

Comme pour HomeKit, **une caméra d'une pièce protégée n'est jamais servie** : `DESCRIBE` répond `403`, et un
flux en cours est coupé dans les 5 s si la pièce devient protégée. Pas de demande d'approbation (une
approbation valide un ordre, pas un regard).

## Sécurité

- Mot de passe **Digest** (MD5, le mot de passe ne circule pas). Le nonce est celui du serveur, pas d'une
  connexion (VLC reçoit le défi sur une connexion et répond sur la suivante) : il change toutes les heures,
  le précédent reste bon, une réponse juste sur un nonce plus ancien est redemandée (`stale`) ; seule `OPTIONS`
  répond sans. Trois mauvais mots de passe et la connexion est fermée, une seconde d'attente après chacun.
  Aucune caméra n'est ouverte avant le bon mot de passe.
- Bornes : 8 clients à la fois, en-têtes de 8 Kio, corps de 4 Kio, 60 s de silence avant `PLAY`, 10 s pour
  qu'un client prenne une image.
- Le RTSP n'est pas chiffré : la vidéo circule en clair sur le réseau local, comme celle de la plupart des
  caméras. À ne pas ouvrir sur Internet.

## ONVIF, pour UniFi Protect

Protect n'adopte que des caméras **ONVIF** (Profile S). Une caméra avec un `onvif_port` en devient une :

- **Découverte** (WS-Discovery) : Moli répond aux `Probe` d'un enregistreur (types
  `NetworkVideoTransmitter` ou `Device`, ou aucun) sur 239.255.255.250:3702, et dit `Hello` au démarrage.
  Seules les adresses IPv4 privées sont servies, 20 réponses par seconde au plus. L'identité (UUID, numéro
  de série) dérive de l'identifiant Moli : stable d'un redémarrage à l'autre.
- **Services** (SOAP 1.2, `http://<hôte>:<onvif_port>/onvif/device_service` et `/onvif/media_service`) :
  date et heure, informations, capacités, services, étendues, nom d'hôte ; deux profils (`main` et `sub`,
  Protect en attend deux ; tous deux le flux unique de la caméra), sources et encodeurs vidéo, adresses du
  flux (`rtsp://<hôte>:8554/<path>`) et de l'image. Le reste répond `ActionNotSupported`.
- **Image fixe** : `/onvif/snapshot.jpg`, en HTTP Digest (`qop=auth`), depuis `hub.camera_image`. Sans
  elle, Protect montre un logo à la place de la caméra.
- **Identifiants** : les mêmes que le RTSP. Une requête qui en porte (WS-Security `UsernameToken`, empreinte
  ou texte, ou HTTP Digest) doit porter les bons, sinon `NotAuthorized` : Protect le dit dès l'adoption. Sans
  identifiants, les descriptions sont données (elles ne contiennent rien de secret : le flux qu'elles
  désignent demande le mot de passe), l'image non. Un `Created` à plus de 15 minutes de l'horloge de Moli est
  refusé.
- Une caméra d'une pièce protégée ne répond pas (`NotAuthorized`, image `403`).

Protect identifie une caméra par son adresse MAC : une seule caméra ONVIF par adresse IP de Moli. Pour en
servir plusieurs, il faudrait une interface réseau virtuelle (macvlan) par caméra.

**Adopter dans Protect** : *Settings → System* (ou *Devices*) *→ Third-Party Cameras*, activer la découverte ;
la caméra apparaît (nom, fabricant « Moli ») ; *Adopt*, avec l'utilisateur `moli` et le mot de passe RTSP.
Ce que Protect en fait : direct, enregistrement, frise ; pas de sonnerie ni de réponse au visiteur (ce n'est
pour lui qu'une caméra), pas de son.

## L'heure incrustée

Écrire la date et l'heure dans l'image demande de **décoder puis recoder** chaque image : c'est ce que Moli
s'interdit (pas de ffmpeg, pas de transcodage, budget de performance), et un encodeur H.264 tournant en continu
coûterait une bonne part d'un cœur, pour une image moins nette. Pas nécessaire non plus : Protect horodate
lui-même ce qu'il enregistre (frise, lecture, export), et Moli lui donne l'heure exacte (rapports RTCP,
`GetSystemDateAndTime`).

## Limites et essais

- Essayé (`src/tests.rs`) : un client RTSP de bout en bout contre la surface et une fausse caméra (mot de
  passe, VLC qui répond sur une nouvelle connexion, SDP, TCP et UDP, rapports RTCP, flux qui démarre sur une
  image clé au numéro de séquence et à l'horodatage annoncés, sans perte, et qui continue quand le flux de la
  caméra s'arrête et est rouvert ; mauvais mots de passe, chemin inconnu, pièce protégée) ; 21 minutes d'une
  source partagée qui lâche ses spectateurs à 10 minutes, en temps accéléré : chaque image une fois, aucune
  manquante ; l'adoption ONVIF (description sans identifiants, WS-Security juste et faux, profils, adresses,
  capacités, action inconnue, image en Digest, pièce protégée) ; une sonde WS-Discovery ; l'empreinte
  WS-Security de l'exemple de la spécification ONVIF.
- Essayé en vrai : VLC sur la sonnette HomeKit (image en 0,1 s).
