# Skill Alexa « maison connectée » pour Moli

Les appareils de Moli dans l'app Alexa : « Alexa, allume la télé », « Alexa, mets le salon à
30 % », depuis une Echo ou une enceinte avec Alexa intégrée (Sonos…), sans « demande à Moli ».
Une skill **privée** (mode développement) : elle ne sert qu'au compte Amazon qui la crée, sans
publication ni validation par Amazon.

```
enceinte → Alexa (Amazon) → AWS Lambda (ce dossier) → https://alexa.example.org/api/alexa → Moli
```

- **Moli** fait tout le protocole (Smart Home API v3, `crates/moli-api/src/alexa_home.rs`) :
  découverte, état, ordres. La Lambda ne fait que relayer, avec un jeton partagé.
- **Ce qu'Alexa voit** : chaque point `on_off` inscriptible des appareils choisis devient un
  appareil, avec la luminosité, la couleur et la température de blanc d'une lampe, le volume et la
  sourdine d'une télé. Catégorie : télé (volume), lampe, prise (puissance mesurée) ou interrupteur.
  Un variateur sans interrupteur (Z-Wave : un seul niveau, 0 = éteint) est une lampe : son niveau
  sert d'interrupteur (allumer = plein, éteindre = 0) et se règle en pourcentage.
- **La garde s'applique** : un ordre d'Alexa part en origine `api`, acteur `alexa`, comme ceux de
  HomeKit. Retenu (pièce protégée, heures calmes, machine), il est dit fait à Alexa et attend
  l'approbation dans le tableau de bord.
- **Ce qu'Internet atteint** : le nom donné à Amazon (`[alexa] host`) ne répond que
  `/api/alexa`, et seulement avec le jeton ; tout le reste répond 404. Les autres noms de Moli
  restent refusés depuis Internet.

## 1. Moli

```toml
[server]
# Le reverse proxy du réseau (un NAS) qui reçoit le 443 : Moli juge alors chaque requête sur le
# vrai client (sans cette ligne, Internet passerait pour la maison).
trusted_proxies = ["192.168.1.x"]

[alexa]
host = "alexa.example.org"          # ajouté de lui-même aux noms autorisés
```

Les appareils qu'Alexa voit et le nom qu'elle entend se règlent dans le tableau de bord,
*Réglages → Intégrations*, tableau **Assistants vocaux** (commun avec HomeKit). Par défaut : tous
les vrais appareils, pas les interrupteurs virtuels des assistants (`helpers`, une présence par
exemple). Le **nom vocal** (« Télé » pour une télé nommée par son modèle) vaut pour Alexa et
HomeKit ; vide, c'est le nom de l'appareil dans Moli. Après un changement : « Alexa, découvre mes
appareils ».

Le jeton partagé, tiré au hasard et rangé dans le coffre, jamais tapé ni affiché :

```sh
head -c 30 /dev/urandom | base64 | tr -d '/+=\n' | tee jeton-alexa.txt \
  | docker exec -i moli-os /moli-os secrets set alexa-skill token
```

Garder `jeton-alexa.txt` le temps de le coller dans la Lambda (étape 3), puis l'effacer.
Redémarrer Moli.

## 2. Le nom public

- DNS : `alexa.example.org` vers l'adresse publique de la maison.
- Reverse proxy : `alexa.example.org` (HTTPS, port 443, vrai certificat) vers `http://<moli>:8790`,
  ouvert à Internet. Le nom du tableau de bord, lui, reste limité au réseau local.

Vérifier depuis le réseau local, en passant par le proxy : `https://alexa.example.org/` doit
répondre 404, et `/api/alexa` sans jeton 401.

## 3. La skill et la Lambda

Avec **le même compte Amazon** que l'app Alexa des enceintes.

1. **Console développeur Alexa** (developer.amazon.com/alexa/console/ask) : *Create Skill*,
   nom « Moli », langue **French (FR)**, modèle **Smart Home**, hébergement *Provision your own*.
   Noter le **Skill ID** (`amzn1.ask.skill.…`).
2. **AWS** (console.aws.amazon.com), région **Europe (Irlande) `eu-west-1`** (celle des skills en
   français) : *Lambda* → *Create function* → *Author from scratch*, nom `moli-alexa`, runtime
   **Python 3.12**.
   - Coller `lambda_function.py` dans l'éditeur, *Deploy*.
   - *Configuration* → *Environment variables* : `MOLI_URL` = `https://alexa.example.org/api/alexa`,
     `MOLI_TOKEN` = le jeton.
   - *Configuration* → *General* : *Timeout* 8 s.
   - *Add trigger* → **Alexa** → *Alexa Smart Home*, avec le Skill ID (seule cette skill peut
     l'appeler).
   - Noter l'**ARN** de la fonction (`arn:aws:lambda:eu-west-1:…:function:moli-alexa`).
3. **Retour à la skill**, *Smart Home* : *Default endpoint* = l'ARN, cocher *Europe and India*
   avec le même ARN. *Save*.
4. **Liaison de compte** (obligatoire pour une skill maison connectée) par *Login with Amazon* :
   - developer.amazon.com → *Login with Amazon* → *Create a New Security Profile* (nom « Moli »).
     Noter son **Client ID** et son **Client Secret**.
   - Dans la skill, *Account Linking* : *Auth Code Grant* ;
     *Web Authorization URI* `https://www.amazon.com/ap/oa` ;
     *Access Token URI* `https://api.amazon.com/auth/o2/token` ;
     Client ID et Secret du profil ; *Client Authentication Scheme* : *HTTP Basic* ;
     *Scope* : `profile:user_id`. *Save*.
   - Copier les **Alexa Redirect URLs** affichées en bas, et les coller dans le profil Login with
     Amazon : *Web Settings* → *Allowed Return URLs*.

   Moli ne lit pas ce jeton Amazon : ce qui protège Moli, c'est le jeton partagé avec la Lambda,
   que seule la skill peut déclencher.

## 4. L'app Alexa

*Plus* → *Skills et jeux* → *Vos skills* → *Dév.* → **Moli** → *Activer* → se connecter avec son
compte Amazon → *Découvrir les appareils*. Les appareils de Moli apparaissent ; les ranger dans
les pièces d'Alexa, créer des routines (« Alexa, soirée film »).

Un appareil ajouté plus tard dans Moli : « Alexa, découvre mes appareils ».

## Dépannage

- *Le serveur ne répond pas* dans l'app : regarder les journaux de la Lambda (CloudWatch) ;
  « Moli injoignable » = le nom public ou le proxy ; un 401 = jeton différent des deux côtés.
- Dans les journaux de Moli : `alexa: token refused` (jeton), `stranger refused` (une requête
  arrivée par un autre nom que celui de `[alexa] host`).
