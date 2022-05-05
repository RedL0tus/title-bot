title-bot
=========

A simple Telegram bot for changing chat titles, but runs on Cloudflare Workers.

Usage
-----

`/echo` - Let the bot say something.  
`/start` - Prints help information.  
`/status` - Prints current settings.  
`/enable` - Enable the bot for the group.  
`/disable` - Disable the bot for the group.  
`/set_template [string]` - Set title template.  
`/set_delimiter [string]` - Set the delimiter between segments of the title template.  
`/set_timezone [timezone]` - Set the timezone of the bot.  
`/push [string]` - Push a new segment to the end of the title template.  
`/push_front [string]` - Push a new segment to the start of the title template.  
`/pop` - Remove a segment of the title template at the end of the title template.  
`/pop_front` - Remove a segment of the title template at the start of the title template.


Deployment
----------

1. Modify [`wrangler.toml`](wrangler.toml) to your needs. NOTE: The domain of the bot should not contain `_`(underscore), otherwise Telegram Bot API would say the Webhook endpoint has an invalid SSL certificate.
2. Retrieve your API token from [@BotFather](https://t.me/BotFather) and upload to Cloudflare.
```bash
wrangler secret put API_TOKEN
```
3. Deploy the bot.
```bash
wrangler deploy
```
4. Send a GET request to the URL of your deployed bot. The bot will send the required request to the Bot API for setting up its webhook.