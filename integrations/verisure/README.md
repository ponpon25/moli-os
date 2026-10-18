# Verisure

- Origine : l'API web de l'application mobile (`customers.securitasdirect.fr/owa-api/graphql` pour
  la France), GraphQL. Rien n'est publié par Verisure : le protocole vient du client ouvert
  `guerrerotook/securitas-direct-new-api` (Apache-2.0), relu et réécrit ici, pas copié.
- `fixtures/installations.json` : **un exemple écrit à la main** d'après le format connu de la liste
  d'installations, pas une réponse capturée sur un vrai compte. Rejoué par les tests de
  `crates/moli-verisure/src/model.rs`.
- Prudence : une seule connexion par mot de passe (un échec compte pour un verrouillage du compte,
  marqué `login_locked` dans le coffre jusqu'à ce qu'un humain le lève), jeton de rafraîchissement
  rangé dès qu'il arrive, requêtes espacées de 2 s, lecture de la centrale toutes les 2 minutes au
  plus vite, attente de 10 minutes après un blocage du pare-feu.
- Armer et désarmer sont des points de sémantique `control` : la garde les retient pour un humain,
  y compris depuis le dashboard du foyer sans code.
- Reste : jamais testé sur une vraie centrale à la livraison de ce paquet. Pas d'état de chaque
  ouverture (l'API ne le donne pas), pas de forçage d'armement, pas de périmètre ni d'annexe en
  commande (leur état se lit). Une connexion de Moli peut déconnecter la session d'une autre
  application du même compte (Jeedom) : un utilisateur Verisure à part pour Moli est préférable.
