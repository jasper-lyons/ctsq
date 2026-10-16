Rails.application.routes.draw do
  get '/about', to: 'static#about'
  get '/contact', to: 'static#contact'
  post '/login', to: 'sessions#create'
  get '/missing', to: 'ghosts#boo'
  resources :gigs, only: %i[index show]
end
